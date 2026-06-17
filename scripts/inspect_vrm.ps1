Add-Type -AssemblyName System.IO.Compression.FileSystem

$vrm = 'C:\Users\pexisgle\Desktop\works\ene\assets\characters\Alicia\AliciaSolid.vrm'
$z = [System.IO.Compression.ZipFile]::OpenRead($vrm)
$entry = $z.Entries | Where-Object { $_.Name -like '*.gltf' -and $_.Name -notlike 'data:*' } | Select-Object -First 1
Write-Output "gltf: $($entry.Name)"
$r = New-Object System.IO.StreamReader($entry.Open())
$json = $r.ReadToEnd()
$r.Close()
$z.Dispose()
$d = $json | ConvertFrom-Json

$ext = $d.extensions.VRMC_vrm
if ($ext) {
    $exps = $ext.expressions
    foreach ($cat in @('preset', 'custom')) {
        $bucket = $exps.$cat
        if ($bucket) {
            Write-Output "--- $cat ---"
            $bucket.PSObject.Properties | ForEach-Object {
                $name = $_.Name
                $def = $_.Value
                $cnt = @($def.morphTargetBinds).Count
                Write-Output "  ${name}: $cnt binds"
                $def.morphTargetBinds | Select-Object -First 4 | ForEach-Object {
                    Write-Output "    node=$($_.node) idx=$($_.index)"
                }
            }
        }
    }
}

Write-Output "--- meshes with morph targets ---"
for ($i = 0; $i -lt $d.meshes.Count; $i++) {
    $m = $d.meshes[$i]
    for ($j = 0; $j -lt $m.primitives.Count; $j++) {
        $p = $m.primitives[$j]
        if ($p.targets) {
            $meshName = if ($m.name) { $m.name } else { "(unnamed)" }
            Write-Output "  mesh[$i].primitives[$j] (${meshName}): $($p.targets.Count) targets"
            for ($k = 0; $k -lt $p.targets.Count; $k++) {
                $t = $p.targets[$k]
                $extras = ""
                if ($t.extras) { $extras = " extras=$($t.extras | ConvertTo-Json -Compress)" }
                Write-Output "    [$k] POSITION=$($t.POSITION) NORMAL=$($t.NORMAL) TANGENT=$($t.TANGENT)${extras}"
            }
        }
    }
}
