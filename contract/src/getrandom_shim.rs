//! Custom getrandom shim for wasm32.
//! k256 needs getrandom but we only do verification (no key generation).
//! This shim provides a zero-fill implementation.

fn custom_getrandom(dest: &mut [u8]) -> Result<(), getrandom::Error> {
    dest.fill(0);
    Ok(())
}

getrandom::register_custom_getrandom!(custom_getrandom);
