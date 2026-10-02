"""Scratch probe: decode item box inventory entries from a JSON dump.

Usage: python probe_itembox.py <dump.json> <rszmhrise.json>
"""
import json
import sys


def murmur3_32(data: bytes, seed: int = 0xFFFFFFFF) -> int:
    c1, c2 = 0xCC9E2D51, 0x1B873593
    length = len(data)
    h1 = seed
    rounded = length & ~3
    for i in range(0, rounded, 4):
        k1 = int.from_bytes(data[i : i + 4], "little")
        k1 = (k1 * c1) & 0xFFFFFFFF
        k1 = ((k1 << 15) | (k1 >> 17)) & 0xFFFFFFFF
        k1 = (k1 * c2) & 0xFFFFFFFF
        h1 ^= k1
        h1 = ((h1 << 13) | (h1 >> 19)) & 0xFFFFFFFF
        h1 = (h1 * 5 + 0xE6546B64) & 0xFFFFFFFF
    k1 = 0
    tail = data[rounded:]
    if len(tail) >= 3:
        k1 ^= tail[2] << 16
    if len(tail) >= 2:
        k1 ^= tail[1] << 8
    if len(tail) >= 1:
        k1 ^= tail[0]
        k1 = (k1 * c1) & 0xFFFFFFFF
        k1 = ((k1 << 15) | (k1 >> 17)) & 0xFFFFFFFF
        k1 = (k1 * c2) & 0xFFFFFFFF
        h1 ^= k1
    h1 ^= length
    h1 ^= h1 >> 16
    h1 = (h1 * 0x85EBCA6B) & 0xFFFFFFFF
    h1 ^= h1 >> 13
    h1 = (h1 * 0xC2B2AE35) & 0xFFFFFFFF
    h1 ^= h1 >> 16
    return h1


def main() -> None:
    dump_path, rsz_path = sys.argv[1], sys.argv[2]
    with open(rsz_path, encoding="utf-8") as f:
        rsz = json.load(f)
    field_map = {}
    for key, t in rsz.items():
        if key == "0" or not isinstance(t, dict):
            continue
        tname = (t.get("name") or f"type_{key}").split(".")[-1]
        for field in t.get("fields", []):
            field_map.setdefault(murmur3_32(field["name"].encode()), []).append((tname, field["name"]))

    def fname(h: int) -> str:
        names = field_map.get(h)
        return names[0][1] if names else f"field_{h:08x}"

    def cname(h: int) -> str:
        t = rsz.get(f"{h:08x}")
        return t.get("name") if t and t.get("name") else f"class_{h:08x}"

    with open(dump_path, encoding="utf-8") as f:
        dump = json.load(f)

    def find_class(value, want_suffix, out):
        if isinstance(value, dict):
            if "hash" in value and "fields" in value:
                if cname(int(value["hash"], 16)).endswith(want_suffix):
                    out.append(value)
                for field in value["fields"]:
                    find_class(field.get("value"), want_suffix, out)
            else:
                for v in value.values():
                    find_class(v, want_suffix, out)
        elif isinstance(value, list):
            for v in value:
                find_class(v, want_suffix, out)

    boxes = []
    find_class(dump, "ItemBox", boxes)
    box = boxes[0]
    inv = None
    for f in box["fields"]:
        if fname(int(f["hash"], 16)) == "_InventoryList":
            inv = f["value"]["values"]
    print("entry class:", cname(int(inv[0]["hash"], 16)))
    print("entry fields:", [fname(int(f["hash"], 16)) for f in inv[0]["fields"]])

    nonempty = []
    for i, element in enumerate(inv):
        if element.get("kind") != "class":
            continue
        parts = []
        for f in element["fields"]:
            name = fname(int(f["hash"], 16))
            v = f["value"]
            if isinstance(v, dict) and v.get("kind") == "scalar":
                parts.append((name, v.get("value"), int(v["hex"], 16) if v.get("hex") else 0))
        # entry is non-empty when any scalar is nonzero
        if any(value for _, value, _ in parts):
            nonempty.append((i, cname(int(element["hash"], 16)), parts))
    print("non-empty entries:", len(nonempty))
    for i, cls, parts in nonempty[:10]:
        text = ", ".join(f"{name}={display}" for name, display, _ in parts)
        print(f"  [{i}] {cls.split('.')[-1]}: {text}")


if __name__ == "__main__":
    main()
