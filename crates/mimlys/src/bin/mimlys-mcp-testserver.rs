//! The scripted MCP server of the M5 tests (`mimlys-mcp::testserver`).
//! Built only with the `test-bins` feature; never part of a release.

fn main() {
    mimlys_mcp::testserver::main();
}
