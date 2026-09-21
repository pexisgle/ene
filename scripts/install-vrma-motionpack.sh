#!/bin/sh
# Downloads the VRoid VRMA motion pack and places it where ene looks for the
# character's clips.
#
# The motion pack is an install asset: it is not committed to this repository,
# and its terms forbid redistributing the clips in an extractable form. The pack
# is published by pixiv as a BOOTH download, so the URL is a direct link to the
# archive on a source you are allowed to use.
#
# Usage:
#   scripts/install-vrma-motionpack.sh --url URL [--destination DIR] [--force]
#
# The URL may also be given as VRMA_MOTIONPACK_URL. Default destination is
# <repository>/assets/motions, the development location;
# apps/ene-body/README.md lists the full search order.
set -eu

pack_clips="VRMA_01.vrma VRMA_02.vrma VRMA_03.vrma VRMA_04.vrma VRMA_05.vrma VRMA_06.vrma VRMA_07.vrma"
# PoseHint -> clip, matching ene_body::motion::DEFAULT_POSE_CLIPS.
hint_clips="VRMA_06.vrma VRMA_02.vrma VRMA_01.vrma VRMA_07.vrma VRMA_03.vrma"

usage() {
    cat <<'EOF'
usage: install-vrma-motionpack.sh --url URL [--destination DIR] [--force]

  -u, --url URL           archive to download (or set VRMA_MOTIONPACK_URL)
  -d, --destination DIR   directory the clips are placed in
                          (default: <repository>/assets/motions)
  -f, --force             overwrite clips that are already placed
  -h, --help              show this help

The pack is a BOOTH download: use the direct archive link of your own download.
EOF
}

url="${VRMA_MOTIONPACK_URL:-}"
destination=""
force=0

while [ $# -gt 0 ]; do
    case "$1" in
        -u|--url) url=$2; shift 2 ;;
        -d|--destination) destination=$2; shift 2 ;;
        -f|--force) force=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ -z "$url" ]; then
    echo "no archive URL: pass --url or set VRMA_MOTIONPACK_URL" >&2
    usage >&2
    exit 2
fi

if [ -z "$destination" ]; then
    script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
    destination=$(dirname -- "$script_dir")/assets/motions
fi

temporary=""
staging=""
cleanup() {
    if [ -n "$temporary" ] && [ -f "$temporary" ]; then rm -f -- "$temporary"; fi
    if [ -n "$staging" ] && [ -d "$staging" ]; then rm -rf -- "$staging"; fi
    :
}
trap cleanup EXIT HUP INT TERM

temporary=$(mktemp "${TMPDIR:-/tmp}/VRMA_MotionPack-XXXXXX.zip")
echo "downloading $url"
if command -v curl >/dev/null 2>&1; then
    curl --fail --location --silent --show-error --output "$temporary" -- "$url"
elif command -v wget >/dev/null 2>&1; then
    wget --quiet --output-document="$temporary" -- "$url"
else
    echo "neither curl nor wget is available to download $url" >&2
    exit 1
fi
if [ ! -s "$temporary" ]; then
    echo "$url produced an empty download" >&2
    exit 1
fi

# Read the clips into a staging directory, leaving the archive closed before
# anything is written into the motion directory.
staging=$(mktemp -d "${TMPDIR:-/tmp}/vrma-motionpack-XXXXXX")
if command -v unzip >/dev/null 2>&1; then
    status=0
    # A pattern without a slash is matched against the file name, so this finds
    # the clips whether the archive nests them under VRMA_MotionPack/vrma/ or
    # keeps them at its root.
    unzip -j -q -o -- "$temporary" 'VRMA_*.vrma' -d "$staging" || status=$?
    # 11 is "no matching files", which is reported below like any empty archive.
    if [ "$status" -ne 0 ] && [ "$status" -ne 11 ]; then
        echo "the download could not be read (unzip exit $status)" >&2
        exit 1
    fi
elif command -v python3 >/dev/null 2>&1; then
    python3 - "$temporary" "$staging" <<'PY'
import os
import re
import shutil
import sys
import zipfile

archive, staging = sys.argv[1], sys.argv[2]
name_pattern = re.compile(r"^VRMA_(\d{1,2})\.vrma$", re.IGNORECASE)
try:
    handle = zipfile.ZipFile(archive)
except (zipfile.BadZipFile, OSError) as error:
    sys.stderr.write("the download is not a readable zip archive: %s\n" % error)
    sys.exit(1)
with handle:
    for entry in handle.infolist():
        # The archive nests the clips under VRMA_MotionPack/vrma/; only the file
        # name identifies them.
        match = name_pattern.match(os.path.basename(entry.filename))
        if match is None:
            continue
        target = os.path.join(staging, "VRMA_%02d.vrma" % int(match.group(1)))
        if os.path.exists(target):
            continue
        with handle.open(entry) as source, open(target, "wb") as sink:
            shutil.copyfileobj(source, sink)
PY
else
    echo "neither unzip nor python3 is available to read the download" >&2
    exit 1
fi

placed=""
kept=""
available=0
mkdir -p -- "$destination"
for name in $pack_clips; do
    staged="$staging/$name"
    if [ ! -f "$staged" ]; then
        continue
    fi
    available=$((available + 1))
    target="$destination/$name"
    if [ -f "$target" ] && [ "$force" -eq 0 ]; then
        kept="$kept $name"
        continue
    fi
    # Write beside the target and rename, so a running body never reads a
    # half-written clip.
    cp -- "$staged" "$target.part"
    mv -f -- "$target.part" "$target"
    placed="$placed $name"
done

if [ "$available" -eq 0 ]; then
    echo "the download holds no VRMA_*.vrma entries; is this the VRMA_MotionPack archive?" >&2
    exit 1
fi

missing=""
for name in $pack_clips; do
    if [ ! -f "$destination/$name" ]; then
        missing="$missing $name"
    fi
done

echo "motion directory: $destination"
if [ -n "$placed" ]; then echo "placed:$placed"; fi
if [ -n "$kept" ]; then echo "kept (already present):$kept"; fi
if [ -n "$missing" ]; then echo "missing from the download:$missing"; fi

covered=0
for name in $hint_clips; do
    if [ -f "$destination/$name" ]; then
        covered=$((covered + 1))
    fi
done
if [ "$covered" -eq 0 ]; then
    echo "no activity-hint clip is placed; the body would keep its staged pose" >&2
    exit 1
fi
echo "activity hints covered: $covered/5"
echo "verify with: cargo run -p ene-body --bin ene-body-asset-probe -- <path to a VRM 1.0 avatar> \"$destination\""
