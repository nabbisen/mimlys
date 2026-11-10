A store written by mimlys 0.8.0 (tag `0.8.0`, commit `2be4c7b`), for the
REQ-PERS-006 test of handoff 009b (`rusqlite` 0.41 must open it).

Made with 0.8.0's own `mimlys-persist` (its `store::tests::sequence` helper, bundled SQLite of
`rusqlite` 0.38): two tasks (one with eight steps and an approval, one with three steps), schema
version 2. The files were copied while the connection was still open, so the data is in
`mimlys.db-wal` (about 650 KB) and `mimlys.db` is the header page only. `mimlys.db-shm` was not
kept: SQLite rebuilds it. Do not regenerate it with a newer version; its point is its origin.
