"""Scratch probe: search RSZ dictionary for equipment-related types and check
which of them appear in a save dump.

Usage: python probe_equipment.py <dump.json> <rszmhrise.json>
"""
import json
import sys

from probe_itembox import murmur3_32


def main() -> None:
    dump_path, rsz_path = sys.argv[1], sys.argv[2]
    with open(rsz_path, encoding="utf-8") as f:
        rsz = json.load(f)

    keywords = ["EquipBox", "Talisman", "MySet", "EquipInventory", "PlArmorSet",
                "ArmorSet", "EquipData", "PlEquip", "ProvisionalEquip", "EquipParam"]
    candidates = {}
    for key, t in rsz.items():
        if key == "0" or not isinstance(t, dict):
            continue
        name = t.get("name") or ""
        if name.startswith("snow.") and any(k.lower() in name.lower() for k in keywords):
            candidates[name] = key

    print(f"=== {len(candidates)} candidate types (name -> dict key) ===")
    for name, key in sorted(candidates.items()):
        fields = rsz[key].get("fields", [])
        print(f"  {name} [{key}] ({len(fields)} fields)")

    # Now check which class hashes actually appear in the save dump.
    with open(dump_path, encoding="utf-8") as f:
        dump = json.load(f)

    key_by_name = {name: key for name, key in candidates.items()}
    found: dict = {}

    def walk(value, path):
        if isinstance(value, dict):
            if "hash" in value and "fields" in value:
                class_hash = value["hash"]
                bare = class_hash[2:] if class_hash.startswith("0x") else class_hash
                if bare in candidates.values():
                    name = rsz[bare]["name"]
                    stats = found.setdefault(name, {"count": 0, "paths": []})
                    stats["count"] += 1
                    if len(stats["paths"]) < 2:
                        stats["paths"].append(path)
                for field in value["fields"]:
                    walk(field.get("value"), f"{path}.{field['hash']}")
            else:
                for v in value.values():
                    walk(v, path)
        elif isinstance(value, list):
            for v in value:
                walk(v, path)

    for entry in dump["payload"]["entries"]:
        walk(entry["class"], f'@{entry["native_hash"]}')

    print(f"\n=== classes present in the save ({len(found)}) ===")
    for name, stats in sorted(found.items()):
        print(f"  {name} x{stats['count']}  e.g. {stats['paths'][0][:100]}")


if __name__ == "__main__":
    main()
