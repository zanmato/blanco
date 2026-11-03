fn main() {
    // `rust_i18n::i18n!` reads the locale files while expanding, which Cargo
    // does not track, so an edited translation would otherwise not rebuild.
    println!("cargo:rerun-if-changed=locales");
}
