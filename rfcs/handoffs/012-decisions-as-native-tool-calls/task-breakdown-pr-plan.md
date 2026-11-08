# Handoff 012 — PR plan (0.9)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 012-1 | The adapter: `decision_transport`, the probe, the functions and their schemas, the strict reply and its mapping, `decision.received` fields; tests with a local stand-in | — | M |
| 012-2 | The context per transport (native turns, no trailing assistant, instructions per transport, the budget counting `tools`); the fake path in both transports | 012-1 | M |
| 012-3 | Run B on llama.cpp and Ollama (no code); the numbers to the architect | 012-2 | S |
| 012-4 | Configuration, `cli.md`, README and getting started after run B; CHANGELOG | 012-3 | S |
| — | Architect: LIM-C1 and the M4 report's run B; 0.9's release review | 012-4 | — |
