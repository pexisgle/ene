import zipfile, json, sys

vrm_path = r'C:\Users\pexisgle\Desktop\works\ene\assets\characters\Alicia\AliciaSolid.vrm'
with zipfile.ZipFile(vrm_path) as z:
    names = z.namelist()
    gltf_files = [n for n in names if n.endswith('.gltf') and not n.startswith('data:')]
    if not gltf_files:
        print('No .gltf file found in archive. Files:', [n for n in names[:20]])
        sys.exit(1)
    for gf in gltf_files:
        print('=== Reading', gf, '===')
        d = json.loads(z.read(gf))
        ext = d.get('extensions', {}).get('VRMC_vrm', {})
        if not ext:
            print('  (no VRMC_vrm)')
            continue
        exps = ext.get('expressions', {})
        for cat in ('preset', 'custom'):
            bucket = exps.get(cat, {})
            if not bucket:
                continue
            print(f'  --- {cat} ({len(bucket)} entries) ---')
            for name, defn in bucket.items():
                binds = defn.get('morphTargetBinds', [])
                print(f'    {name!r:25s}  {len(binds)} binds')
                for b in binds:
                    print(f'        node={b.get("node")}, index={b.get("index")}, weight={b.get("weight", 1.0)}')
        break
    print('--- meshes with morph targets ---')
    for i, m in enumerate(d.get('meshes', [])):
        for j, p in enumerate(m.get('primitives', [])):
            targets = p.get('targets', [])
            if targets:
                print(f'  mesh[{i}].primitives[{j}] ({m.get("name", "")}): {len(targets)} morph targets')
                # show first 10 target names from extras if any
                for k, t in enumerate(targets[:10]):
                    extras = t.get('extras', {})
                    print(f'      [{k}] extras={extras}')
