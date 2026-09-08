#!/usr/bin/env python3
"""Notify ene GUI確認 webhook when an open PR is ready for GUI smoke.

Runs on GitHub Actions (primary) so it still fires when the Grok Bot computer is down.

Mirrors /home/box/ene-gui-smoke-watcher/watch.py fire rules:
  - PR touches apps/ene-stage/ or apps/ene-desktop/
  - All non-skipped checks green
  - First green ever, or first green on a new head after label gui-smoke-issues
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = os.environ.get("REPO", "pexisgle/ene")
STATE_DIR = Path(os.environ.get("STATE_DIR", ".gui-smoke-hook-state"))
STATE_PATH = STATE_DIR / "state.json"
ISSUES_LABEL = "gui-smoke-issues"
GUI_PREFIXES = ("apps/ene-stage/", "apps/ene-desktop/")
RETRY_BACKOFFS_SEC = (1, 2, 4)


def gh_json(args: list[str]):
    raw = subprocess.check_output(["gh", *args], text=True)
    return json.loads(raw) if raw.strip() else None


def load_state() -> dict:
    if STATE_PATH.exists():
        try:
            return json.loads(STATE_PATH.read_text())
        except json.JSONDecodeError:
            pass
    return {"fired_green": {}}


def save_state(state: dict) -> None:
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    STATE_PATH.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")


def checks_all_green(rollup) -> bool:
    if not rollup:
        return False
    saw = False
    for c in rollup:
        name = (c.get("name") or c.get("context") or "").lower()
        if "prune" in name:
            continue
        conclusion = (c.get("conclusion") or "").upper()
        state = (c.get("state") or "").upper()
        if state in {"PENDING", "QUEUED", "IN_PROGRESS", "EXPECTED"} and not conclusion:
            return False
        if conclusion in {"SKIPPED", "CANCELLED"}:
            continue
        if conclusion in {"", "NEUTRAL"}:
            if state in {"SUCCESS", "COMPLETED"}:
                saw = True
                continue
            return False
        if conclusion in {"FAILURE", "FAILED", "TIMED_OUT", "ACTION_REQUIRED", "ERROR"}:
            return False
        if state in {"FAILURE", "FAILED", "ERROR"}:
            return False
        if conclusion in {"SUCCESS", "PASSED"} or state == "SUCCESS":
            saw = True
            continue
        return False
    return saw


def looks_gui_relevant(files: list | None) -> bool:
    if not files:
        return False
    for f in files:
        path = f.get("path") or ""
        if any(h in path for h in GUI_PREFIXES):
            return True
    return False


def pr_has_issues_label(labels: list | None) -> bool:
    for lab in labels or []:
        name = lab.get("name") if isinstance(lab, dict) else str(lab)
        if name == ISSUES_LABEL:
            return True
    return False


def should_fire(pr_num: int, head: str, has_issues: bool, state: dict) -> tuple[bool, str]:
    key = str(pr_num)
    fired = state.setdefault("fired_green", {})
    prior_head = fired.get(key)
    if prior_head == head:
        return False, "already_fired_this_green"
    if has_issues:
        if prior_head is None:
            return False, "issues_label_keep_no_prior_fire"
        return True, "first_green_after_issues"
    if key not in fired:
        return True, "first_green_never_smoked"
    return False, "implicit_pass_skip"


class WebhookTransientError(Exception):
    """5xx / network / timeout after retries — skip this PR, keep the job green."""


def post_webhook(url: str, key: str | None, payload: dict) -> None:
    """POST the GUI webhook.

    Retries HTTP 5xx, URLError, and timeout (~3 attempts, backoff 1s/2s/4s).
    HTTP 4xx is raised immediately (config bug — fail the job).
    After retries are exhausted, raises WebhookTransientError (caller must not
    mark fired_green and must keep the job green).
    """
    data = json.dumps(payload).encode()
    headers = {"Content-Type": "application/json"}
    if key:
        headers["Authorization"] = f"Bearer {key}"
        headers["X-Webhook-Key"] = key

    attempts = len(RETRY_BACKOFFS_SEC)
    last_err: Exception | None = None
    for i in range(attempts):
        req = urllib.request.Request(url, data=data, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                resp.read()
            return
        except urllib.error.HTTPError as e:
            body = e.read()[:200]
            print(f"webhook HTTP {e.code} (attempt {i + 1}/{attempts}): {body!r}", flush=True)
            if 400 <= e.code < 500:
                raise
            last_err = e
        except urllib.error.URLError as e:
            print(f"webhook URLError (attempt {i + 1}/{attempts}): {e}", flush=True)
            last_err = e
        except TimeoutError as e:
            print(f"webhook timeout (attempt {i + 1}/{attempts}): {e}", flush=True)
            last_err = e
        if i + 1 < attempts:
            delay = RETRY_BACKOFFS_SEC[i]
            print(f"retrying webhook in {delay}s", flush=True)
            time.sleep(delay)
    print(f"webhook failed after {attempts} attempts: {last_err}", flush=True)
    raise WebhookTransientError(str(last_err))


def clear_issues_label(pr_num: int) -> None:
    try:
        subprocess.check_call(
            ["gh", "pr", "edit", str(pr_num), "--repo", REPO, "--remove-label", ISSUES_LABEL],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except subprocess.CalledProcessError:
        pass


def main() -> int:
    webhook = (os.environ.get("WEBHOOK_URL") or "").strip()
    if not webhook:
        print("WEBHOOK_URL missing — skip", flush=True)
        return 0
    key = (os.environ.get("WEBHOOK_KEY") or "").strip() or None

    state = load_state()
    prs = gh_json(
        [
            "pr",
            "list",
            "--repo",
            REPO,
            "--state",
            "open",
            "--limit",
            "50",
            "--json",
            "number,title,headRefOid,url,isDraft,statusCheckRollup,files,labels",
        ]
    )
    if not isinstance(prs, list):
        print("unexpected pr list", flush=True)
        return 1

    open_nums = {str(p.get("number")) for p in prs if p.get("number") is not None}
    fg = state.setdefault("fired_green", {})
    for k in list(fg):
        if k not in open_nums:
            del fg[k]

    fired_any = False
    for pr in prs:
        num = pr.get("number")
        head = pr.get("headRefOid") or ""
        if not num or not head:
            continue
        if not checks_all_green(pr.get("statusCheckRollup")):
            continue
        if not looks_gui_relevant(pr.get("files")):
            print(f"skip #{num} non-gui files", flush=True)
            continue
        has_issues = pr_has_issues_label(pr.get("labels"))
        fire, reason = should_fire(num, head, has_issues, state)
        if not fire:
            print(f"skip #{num} {reason}", flush=True)
            continue

        payload = {
            "source": "ene-gui-smoke-hook-actions",
            "repo": REPO,
            "pr": num,
            "title": pr.get("title"),
            "url": pr.get("url"),
            "head": head,
            "reason": reason,
            "isDraft": pr.get("isDraft"),
        }
        print(f"fire GUI webhook #{num} head={head[:8]} reason={reason}", flush=True)
        try:
            post_webhook(webhook, key, payload)
        except urllib.error.HTTPError:
            # 4xx already printed — auth / disabled automation / other config bugs
            return 1
        except WebhookTransientError:
            # 5xx / network after retries: do not mark fired_green; continue
            continue
        except Exception as e:
            print(f"webhook error: {e}", flush=True)
            return 1
        state.setdefault("fired_green", {})[str(num)] = head
        if reason == "first_green_after_issues":
            clear_issues_label(num)
        fired_any = True

    save_state(state)
    if not fired_any:
        print("no fires", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
