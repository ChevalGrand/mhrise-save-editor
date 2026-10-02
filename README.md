# MHRise Save Editor

A Steam-only tool for inspecting, dumping, editing, and repacking *Monster Hunter Rise* (Steam / `win64_save`) save files. Built for editing in-save values (items, counts, and similar) and for transferring data between the three save slots (`data001Slot.bin` … `data003Slot.bin`).

> [!WARNING]
> Always back up your saves (`backup` command) and edit with the game closed. Field mappings were verified against real saves (wallet, points, item box, equipment); unusual edits may still behave unexpectedly — test on a throwaway slot first.

## GUI

```bash
cargo run --release --bin mhrise-save-editor-gui
```

Pick a target save, optionally a source save (for transfers), enter your SteamID64, and use the buttons to read save info, set wallet/points, transfer the item box, or transfer the whole equipment complex (equipment box + talismans + loadout registers + worn pack). Every operation runs in the background and writes a NEW file (`<name>.edited.bin` / `<name>.transferred.bin`) next to the chosen save; swap it in with the game closed.

## Attribution

The DSSS container, Citrus, and RE Engine class-stream handling in `src/crypto/`, `src/format.rs`, `src/payload.rs`, and `src/discover.rs` is ported verbatim from the MIT-licensed [jinghaihan/mhrise-save-converter](https://github.com/jinghaihan/mhrise-save-converter), whose format research builds on [kvasszn/ree-save-editor](https://github.com/kvasszn/ree-save-editor). Everything else in this repository is original to this project.

## Installation

```bash
cargo install --path .
```

## Usage

All commands need your **SteamID64** — the 17-digit numeric ID of the account that owns the save (a numeric Steam profile URL looks like `steamcommunity.com/profiles/<STEAMID64>`). The Curve Index is detected from the save itself.

```bash
# Show header, platform, and checksum status for a save file or directory
mhrise-save-editor inspect <file-or-dir>

# Decrypt + parse a save into a human-readable JSON tree
mhrise-save-editor dump-json data001Slot.bin --steamid64 76561198000000000 -o slot1.json

# Load an edited JSON back onto the save and write a new save file
mhrise-save-editor apply-json data001Slot.bin slot1.json --steamid64 76561198000000000 -o slot1.edited.bin

# Repack a save without changing anything (round-trip check)
mhrise-save-editor repack data001Slot.bin --steamid64 76561198000000000 -o slot1.repacked.bin

# Report whether parse -> encode reproduces the decrypted payload byte-for-byte
mhrise-save-editor roundtrip data001Slot.bin --steamid64 76561198000000000

# Set the wallet amount (and optionally the lifetime money-gained counter)
mhrise-save-editor set-money data001Slot.bin --steamid64 76561198000000000 --value 99999999

# Set the Kamura/Steady point balance
mhrise-save-editor set-points data001Slot.bin --steamid64 76561198000000000 --value 500000

# Structurally diff two saves (fresh vs progressed character), aggregated by field path
mhrise-save-editor diff fresh.bin progressed.bin --steamid64 76561198000000000

# Replace the target character's item box with the source character's item box
# (both saves must belong to the same Steam account)
mhrise-save-editor transfer-items new-character.bin old-character.bin --steamid64 76561198000000000

# Back up an entire save directory (verbatim copy, byte-verified).
# For a real Steam layout the backup goes to Documents\MHR-Save-Backups\<account>\
# so it stays outside the Steam Cloud sync scope.
mhrise-save-editor backup ".../userdata/<account>/1446780/remote/win64_save"

# Restore a backup into a directory; overwriting existing files needs --force
mhrise-save-editor restore ".../Documents/MHR-Save-Backups/<account>/<timestamp>" ".../userdata/<account>/1446780/remote/win64_save" --force
```

The source save is never modified; every writing command produces a new file.

### No save at hand?

`cargo run --example make_fixture -- <dir>` generates a synthetic Steam save (`data001Slot.bin`) for trying every command, with SteamID64 `76561198382766028`.

### Reading `roundtrip` results

- `Structural roundtrip: OK` — the re-encoded payload parses back to the identical tree. This is the property edits rely on.
- `Byte roundtrip: BYTE-IDENTICAL` — best case.
- `Byte roundtrip: DIFFERS` — expected on real saves: the game leaves stale memory bytes in alignment-padding gaps, while a re-encode writes zeros there. Verified on real saves: all differing bytes sit in parser-skipped padding, lengths and structure match exactly, and the game demonstrably accepts zero-filled gaps (this encoding path is what the upstream converter's tested Switch→Steam conversion uses). The decisive acceptance test is loading a `repack`ed save in the game.

### Safety workflow

1. `backup` the `win64_save` directory (byte-verified copy) before any testing.
2. `roundtrip` a core save — expect structural OK.
3. `repack` a copy and load it into the game with Steam in offline mode — it must behave like the original.
4. Only then edit JSON and `apply-json` it, and test the result the same way.
5. If anything looks wrong in game: close the game, `restore` the backup with `--force`.

## How it fits together

| Layer | Module | Role |
| --- | --- | --- |
| Container | `format.rs` | DSSS v2 header, platform flags, MurmurHash3 outer checksum |
| Crypto | `crypto/` | Citrus: SteamID64-derived ECC keys, AES-128-CBC, SHA3-256 block hashes, curve brute-force |
| Payload | `payload.rs` | RE Engine class stream: parse / edit / re-encode with alignment |
| Document | `container.rs` | open → edit → write for Steam core saves |
| Archive | `archive.rs` | byte-verified whole-directory backup / restore |
| JSON | `json.rs` | lossless payload ⇄ JSON dump/load |

### JSON dump rules

`hash`es and `type`s are hex strings. For scalars, `hex` holds the exact little-endian bytes and is **authoritative** when loading; `value` (unsigned-integer view, sizes ≤ 8) and `float` (f32/f64 view) are display-only — the loader refuses a dump whose display fields no longer match `hex`, so an edit in the wrong field fails loudly instead of being silently dropped. Strings carry their exact UTF-16 `units` (plus a lossy `value` for reading).

## License

[MIT](./LICENSE) License © ChevalGrand
