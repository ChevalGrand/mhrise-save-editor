"""Scratch probe: find every field whose value equals the given numbers.

Usage: python probe_find_values.py <dump.json> <rszmhrise.json> <num> [<num> ...]
"""
import json
import sys

from probe_itembox import murmur3_32


def main() -> None:
    dump_path, rsz_path = sys.argv[1], sys.argv[2]
    targets = {int(v) for v in sys.argv[3:]}
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

    hits = []

    def walk(value, path):
        if isinstance(value, dict):
            if "hash" in value and "fields" in value:
                class_hash = value["hash"]
                for field in value["fields"]:
                    fpath = f"{path}.{fname(int(field['hash'], 16))}"
                    v = field.get("value")
                    if isinstance(v, dict) and v.get("kind") == "scalar":
                        text = v.get("value")
                        if text is not None and text.lstrip("-").isdigit():
                            number = int(text)
                            if number in targets:
                                size = v.get("size")
                                hits.append((number, fpath, cname(int(class_hash, 16)), size))
                    walk(v, fpath)
            else:
                for v in value.values():
                    walk(v, path)
        elif isinstance(value, list):
            for v in value:
                walk(v, path)

    for entry in dump["payload"]["entries"]:
        walk(entry["class"], f'@{entry["native_hash"]}')

    for number in sorted(targets):
        matching = [hit for hit in hits if hit[0] == number]
        print(f"=== value {number}: {len(matching)} field(s) ===")
        for _, path, class_name, size in matching[:12]:
            print(f"  {path}  [{class_name}, {size}B]")


if __name__ == "__main__":
    main()
