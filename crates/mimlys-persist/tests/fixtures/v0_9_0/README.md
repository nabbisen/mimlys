A store written by mimlys 0.9.0 (tag `0.9.0`, commit `2bc3b4e`), for the golden tests of
handoff 008c (the workspace's `sha2` moves from 0.10 to 0.11; nothing the product hashes may change).

Made with 0.9.0's own `mimlys-persist` and `sha2` 0.10: one task (three steps), then two snapshot
objects (`a.txt`, 21 bytes; `tools/big.bin`, 70 000 bytes of `i % 251`, which crosses the 64 KiB read
block) and a manifest of three entries, written with `SnapshotFiles`. The database files were copied
while the connection was still open, so the data is in `mimlys.db-wal` and `mimlys.db` is the header
page only. Do not regenerate it with a newer version: its point is its origin.
