// Historical modules remain available to compatibility tests, without a service entry.
console.error('NODE_RUNTIME_RETIRED: use the Rust work-orders API; from the repository root run cargo run --locked --manifest-path business-apps/work-orders/backend/Cargo.toml, or deploy its Rust image.')
process.exitCode = 1
