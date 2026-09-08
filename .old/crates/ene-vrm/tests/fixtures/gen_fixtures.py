#!/usr/bin/env python3
"""Deterministic VRM fixture GLB generator for ene-vrm integration tests.

Run after editing any layout constant below:

    python3 crates/ene-vrm/tests/fixtures/gen_fixtures.py

Writes minimal_vrm0.vrm (VRM 0.x), invalid_root_extension.vrm (a glTF with
no VRM root extension), unsupported_spec.vrm (VRM 1.0 with an unknown
specVersion), and swatch.png next to this script. The models share one
binary layout:
a 64x64 grey PNG (data URI) and 196 bytes of float/byte data holding a
skinned quad (POSITION/NORMAL/TEXCOORD_0/JOINTS_0/WEIGHTS_0), an IBM
block for the four-joint skin (hips/spine/head + leftUpperArm branch),
and index buffers.
"""

import base64
import json
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))

# Node indices shared by both builders. Keep in sync with the JSON trees.
N_HIPS = 0
N_SPINE = 1
N_HEAD = 2
N_LEFT_ARM = 3
N_BODY_QUAD = 4
N_MORPH_QUAD = 5   # legacy only
N_SPRING_A = 6     # legacy only
N_SPRING_B = 7     # legacy only

POSITIONS = [(-0.5, 0.9, -0.1), (0.5, 0.9, -0.1), (0.5, 1.7, 0.1), (-0.5, 1.7, 0.1)]
NORMALS = [(0.0, 0.0, 1.0)] * 4
UVS = [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)]
INDICES = (0, 1, 2, 0, 2, 3)
JOINTS = [(1, 0, 0, 0), (1, 0, 0, 0), (2, 0, 0, 0), (2, 0, 0, 0)]
WEIGHTS = [(1.0, 0.0, 0.0, 0.0)] * 4
IBM_FLAT = (
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    0.0, -0.9, 0.1, 1.0,
)  # column-major glTF IBM: positions are hips-local (y - 0.9, z - 0.1).


def build_png():
    width = height = 64
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        raw += bytes((y * 4) % 256 for _ in range(width))

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def build_bin():
    out = bytearray()
    for p in POSITIONS:
        out += struct.pack("<fff", *p)
    for n in NORMALS:
        out += struct.pack("<fff", *n)
    for uv in UVS:
        out += struct.pack("<ff", *uv)
    for idx in INDICES:
        out += struct.pack("<H", idx)
    for j in JOINTS:
        out += bytes(j)
    for w in WEIGHTS:
        out += struct.pack("<ffff", *w)
    for _ in range(4):
        for value in IBM_FLAT:
            out += struct.pack("<f", value)
        out += struct.pack("<f", value)
    return bytes(out)


def pack_glb(json_obj):
    json_bytes = json.dumps(json_obj, separators=(",", ":")).encode("utf-8")
    bin_chunk = build_bin()
    json_pad = (4 - len(json_bytes) % 4) % 4
    bin_pad = (4 - len(bin_chunk) % 4) % 4
    # GLB chunkLength includes the alignment padding: the glTF parser seeks
    # next-header = current-header + 8 + chunkLength, so an unpadded length
    # leaves the reader mid-pad inside the next chunk type field.
    total = 12 + 8 + len(json_bytes) + json_pad + 8 + len(bin_chunk) + bin_pad
    out = bytearray()
    out += b"glTF" + struct.pack("<II", 2, total)
    out += struct.pack("<I", len(json_bytes) + json_pad) + b"JSON"
    out += json_bytes + b" " * json_pad
    out += struct.pack("<I", len(bin_chunk) + bin_pad) + b"BIN\x00"
    out += bin_chunk + b"\x00" * bin_pad
    assert len(out) == total
    return bytes(out)


def views_and_accessors():
    views = [
        {"buffer": 0, "byteOffset": 0, "byteLength": 48},
        {"buffer": 0, "byteOffset": 48, "byteLength": 48},
        {"buffer": 0, "byteOffset": 96, "byteLength": 32},
        {"buffer": 0, "byteOffset": 128, "byteLength": 12, "target": 34963},
        {"buffer": 0, "byteOffset": 140, "byteLength": 16},
        {"buffer": 0, "byteOffset": 156, "byteLength": 64},
        {"buffer": 0, "byteOffset": 220, "byteLength": 256},
    ]
    accessors = [
        {
            "bufferView": 0,
            "componentType": 5126,
            "count": 4,
            "type": "VEC3",
            "min": [-0.5, 0.9, -0.1],
            "max": [0.5, 1.7, 0.1],
        },
        {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3"},
        {"bufferView": 2, "componentType": 5126, "count": 4, "type": "VEC2"},
        {"bufferView": 3, "componentType": 5123, "count": 6, "type": "SCALAR"},
        {"bufferView": 4, "componentType": 5121, "count": 4, "type": "VEC4"},
        {"bufferView": 5, "componentType": 5126, "count": 4, "type": "VEC4"},
        {"bufferView": 6, "componentType": 5126, "count": 4, "type": "MAT4"},
    ]
    return views, accessors


def png_data_uri():
    with open(os.path.join(HERE, "swatch.png"), "rb") as f:
        png = f.read()
    return "data:image/png;base64," + base64.b64encode(png).decode("ascii")


QUAD_PRIMITIVE = {
    "attributes": {
        "POSITION": 0,
        "NORMAL": 1,
        "TEXCOORD_0": 2,
        "JOINTS_0": 4,
        "WEIGHTS_0": 5,
    },
    "indices": 3,
    "material": 0,
}

HUMANOID_NODES = [
    {"name": "J_Bip_C_Hips", "children": [N_SPINE], "translation": [0.0, 0.9, 0.1]},
    {"name": "J_Bip_C_Spine", "children": [N_HEAD], "translation": [0.0, 0.1, 0.05]},
    {"name": "J_Bip_C_Head", "translation": [0.0, 0.1, 0.0]},
    {"name": "J_Bip_L_UpperArm", "translation": [0.15, 0.2, 0.0]},
    {"name": "BodyQuad", "mesh": 0, "skin": 0, "translation": [-0.35, 0.0, 0.0]},
]


def common_tree(mat_extensions, extensions_used, skinned_body=True):
    views, accessors = views_and_accessors()
    body_node = dict(HUMANOID_NODES[N_BODY_QUAD])
    if not skinned_body:
        body_node.pop("skin", None)
    doc = {
        "asset": {"version": "2.0", "generator": "ene-vrm fixture"},
        "scene": 0,
        "scenes": [{"nodes": [N_HIPS]}],
        "images": [{"uri": png_data_uri()}],
        "samplers": [
            {"magFilter": 9729, "minFilter": 9729, "wrapS": 10497, "wrapT": 10497}
        ],
        "nodes": [
            HUMANOID_NODES[N_HIPS],
            HUMANOID_NODES[N_SPINE],
            HUMANOID_NODES[N_HEAD],
            HUMANOID_NODES[N_LEFT_ARM],
            body_node,
        ],
        "skins": [
            {"joints": [N_HIPS, N_SPINE, N_HEAD, N_LEFT_ARM], "inverseBindMatrices": 6}
        ],
        "meshes": [{"name": "Body", "primitives": [dict(QUAD_PRIMITIVE)]}],
        "materials": [
            {
                "name": "BodyMToon",
                "alphaMode": "OPAQUE",
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 0},
                    "baseColorFactor": [0.8, 0.85, 0.95, 1.0],
                },
                **mat_extensions,
            }
        ],
        "textures": [{"sampler": 0, "source": 0}],
        "buffers": [{"byteLength": 632}],
        "bufferViews": views,
        "accessors": accessors,
    }
    if extensions_used:
        doc["extensionsUsed"] = extensions_used
    return doc


def write_vrm0(path):
    mtoon_material = {
        "extensions": {
            "VRM": {"renderQueue": 3000},
            "KHR_materials_unlit": {},
        },
        "_shader": "VRM/MToon",
    }
    doc = common_tree(mtoon_material, ["KHR_materials_unlit", "VRM"])
    nodes = list(doc["nodes"])
    while len(nodes) <= N_SPRING_B:
        nodes.append({"name": "placeholder"})
    nodes[N_MORPH_QUAD] = {
        "name": "FaceMorphQuad",
        "mesh": 1,
        "skin": 0,
        "translation": [0.35, 0.0, 0.0],
    }
    nodes[N_SPRING_A] = {
        "name": "J_Sec_Hair1_1",
        "children": [N_SPRING_B],
        "translation": [0.0, 1.75, 0.0],
    }
    nodes[N_SPRING_B] = {"name": "J_Sec_Hair1_2", "translation": [0.0, 0.2, 0.0]}
    doc["nodes"] = nodes
    doc["scenes"] = [{"nodes": [N_HIPS]}]
    morph_primitive = dict(QUAD_PRIMITIVE)
    morph_primitive["targets"] = [
        {"POSITION": 0},
        {"POSITION": 0},
        {"POSITION": 0},
    ]
    morph_primitive["extras"] = {"targetWeights": [0.5, 0.25, 0.0]}
    doc["meshes"].append(
        {
            "name": "FaceMorphMesh",
            "extras": {"targetNames": ["happy", "sad", "blink"]},
            "primitives": [morph_primitive],
        }
    )
    doc["extensions"] = {
        "VRM": {
            "meta": {
                "title": "Fixture VRM0",
                "version": "v0.1-test",
                "author": "ene-vrm fixture suite",
                "allowedUserName": "Everyone",
                "violentUssageName": "Disallow",
                "commercialUssageName": "Allow",
                "contactName": "",
                "reference": "",
                "texture": 0,
            },
            "humanoid": {
                "humanBones": [
                    {"bone": "hips", "node": N_HIPS},
                    {"bone": "spine", "node": N_SPINE},
                    {"bone": "head", "node": N_HEAD},
                    {"bone": "leftUpperArm", "node": N_LEFT_ARM},
                ]
            },
            "firstPerson": {
                "firstPersonBone": N_HEAD,
                "firstPersonBoneOffset": [0.0, 0.06, 0.03],
                "meshAnnotations": [],
                "lookAtTypeName": "Bone",
                "lookAtHorizontalInner": {"curve": [], "xRange": 60.0, "yRange": 8.0},
                "lookAtHorizontalOuter": {"curve": [], "xRange": 70.0, "yRange": 10.0},
                "lookAtVerticalDown": {"curve": [], "xRange": 45.0, "yRange": 6.0},
                "lookAtVerticalUp": {"curve": [], "xRange": 50.0, "yRange": 7.0},
                "lookAtBoneName": "J_Bip_C_Head",
            },
            "blendShapeMaster": {
                "blendShapeGroups": [
                    {
                        "name": "Joy",
                        "presetName": "joy",
                        "binds": [{"mesh": 1, "index": 0, "weight": 90.0}],
                    },
                    {
                        "name": "Sorrow",
                        "presetName": "sorrow",
                        "binds": [{"mesh": 1, "index": 1, "weight": 80.0}],
                    },
                    {
                        "name": "Blink_L",
                        "presetName": "blink",
                        "binds": [{"mesh": 1, "index": 2, "weight": 100.0}],
                    },
                ]
            },
            "secondaryAnimation": {
                "boneGroups": [
                    {
                        "stiffiness": 4.0,
                        "gravityPower": 0.01,
                        "gravityDir": [0.0, -1.0, 0.0],
                        "dragForce": 0.3,
                        "hitRadius": 0.02,
                        "bones": [N_SPRING_A, N_SPRING_B],
                        "colliderGroups": [],
                    }
                ],
                "colliderGroups": [],
            },
        }
    }
    with open(os.path.join(HERE, path), "wb") as f:
        f.write(pack_glb(doc))


def write_invalid(path):
    # A plain glTF document: format detection requires a declared VRM root
    # extension, so this must fail with NotVrm rather than rendering empty.
    doc = common_tree({}, [], skinned_body=False)
    with open(os.path.join(HERE, path), "wb") as f:
        f.write(pack_glb(doc))


def write_unsupported_spec(path):
    # Declares VRM 1.0 but a future specVersion the loader does not
    # implement; must surface as UnsupportedFormat so GUIs can label it.
    doc = common_tree({}, ["VRMC_vrm"], skinned_body=False)
    doc["extensions"] = {"VRMC_vrm": {"specVersion": "9.9"}}
    with open(os.path.join(HERE, path), "wb") as f:
        f.write(pack_glb(doc))


if __name__ == "__main__":
    with open(os.path.join(HERE, "swatch.png"), "wb") as f:
        f.write(build_png())
    write_vrm0("minimal_vrm0.vrm")
    write_invalid("invalid_root_extension.vrm")
    write_unsupported_spec("unsupported_spec.vrm")
    print("fixtures written to", HERE)
