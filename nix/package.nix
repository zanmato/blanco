{
  lib,
  craneLib,
  writeShellScript,
  bash,
  util-linux,
  pkg-config,
  alsa-lib,
  fontconfig,
  freetype,
  libxkbcommon,
  libx11,
  libxcb,
  libxcursor,
  libxi,
  libxrandr,
  openssl,
  vulkan-loader,
  wayland,
  zlib,
}:

let
  root = ../.;

  # Sources the build reads. Docs, the website and the Nix files stay out, so
  # editing them does not rebuild Blanco.
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../.cargo
      ../crates
      ../scripts/cxx-limited
    ];
  };

  cargoToml = lib.importTOML ../Cargo.toml;

  # .cargo/config.toml points CXX at scripts/cxx-limited by a relative path,
  # which the dependency build's stripped source does not contain, and whose
  # /usr/bin/env shebang the sandbox cannot run. Cargo's [env] leaves variables
  # that are already set alone, so this takes its place in both builds.
  cxxLimited = writeShellScript "cxx-limited" ''
    export PATH=${lib.makeBinPath [ util-linux ]}:$PATH
    exec ${bash}/bin/bash ${../scripts/cxx-limited} "$@"
  '';

  # GPUI loads these with dlopen, so the linker never records them. They go in
  # an rpath rather than a wrapper's LD_LIBRARY_PATH, which the shell in
  # Blanco's terminal and everything run from it would inherit.
  runtimeLibraries = [
    vulkan-loader
    wayland
    libxkbcommon
    libx11
    libxcb
    libxcursor
    libxi
    libxrandr
  ];

  commonArgs = {
    inherit src;
    pname = "blanco";
    version = cargoToml.workspace.package.version;
    strictDeps = true;
    cargoExtraArgs = "--package blanco";

    nativeBuildInputs = [ pkg-config ];

    buildInputs = [
      alsa-lib
      fontconfig
      freetype
      openssl
      zlib
    ]
    ++ runtimeLibraries;

    CXX_x86_64_unknown_linux_gnu = cxxLimited;
    CXX_aarch64_unknown_linux_gnu = cxxLimited;
  };

  # Every dependency, DuckDB's C++ included, built once and reused until
  # Cargo.lock changes.
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;

    # The workspace tests need database servers, so they run in CI instead.
    doCheck = false;

    NIX_LDFLAGS = "-rpath ${lib.makeLibraryPath runtimeLibraries}";
    # The fixup's patchelf --shrink-rpath keeps only directories of libraries
    # the binary links, which would drop the ones above.
    dontPatchELF = true;

    passthru = { inherit runtimeLibraries; };

    postInstall = ''
      install -Dm644 crates/blanco/resources/blanco.desktop \
        $out/share/applications/blanco.desktop
      install -Dm644 crates/blanco/resources/blanco-duckdb.xml \
        $out/share/mime/packages/blanco-duckdb.xml
      install -Dm644 crates/blanco/assets/images/blanco.svg \
        $out/share/icons/hicolor/scalable/apps/blanco.svg
    '';

    meta = {
      description = "SQL editor and database client";
      homepage = "https://github.com/zanmato/blanco";
      license = lib.licenses.asl20;
      mainProgram = "blanco";
      platforms = lib.platforms.linux;
    };
  }
)
