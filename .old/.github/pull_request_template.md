## Change checklist

- [ ] This change does not add or replace a bundled binary/media asset.
- [ ] If it does, `third_party/assets.json` records the path, SHA-256, source,
      publisher, license, permissions, and distribution path.
- [ ] Redistribution terms and required notices were checked for every changed
      asset.
- [ ] Release packaging includes any notice or license text required by the
      changed asset.
- [ ] `bash scripts/check-asset-provenance.sh` passes.
