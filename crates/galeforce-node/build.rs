// napi-rs build script. Generates the per-platform symbol exports
// that Node's N-API loader needs to find when it `require`s our
// `.node` artifact. Without this, the .node file links but Node
// can't see the exported functions.
fn main() {
    napi_build::setup();
}
