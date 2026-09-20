{
  lib,
  rustPlatform,
  pkg-config,
  cmake,
  webkitgtk_4_1,
  json-glib,
  libxkbcommon,
  wayland,
  libxcb,
  libx11,
  fontconfig,
  freetype,
  vulkan-loader,
  libglvnd,
}:

let
  # Loaded with dlopen at runtime, so the linker never records them in RPATH.
  runtimeLibs = [
    wayland
    vulkan-loader
    libglvnd
  ];
in
rustPlatform.buildRustPackage {
  pname = "zeron";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../apps/zeron
      ../crates
      ../dist/zeron.desktop
      ../dist/zeron.png
      # include_str!'d by unit tests.
      ../edge/src/install.sh
      ../scripts/fixtures
    ];
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  cargoBuildFlags = [
    "-p"
    "zeron"
  ];

  nativeBuildInputs = [
    pkg-config
    cmake
  ];

  buildInputs = [
    webkitgtk_4_1
    json-glib
    libxkbcommon
    wayland
    libxcb
    libx11
    fontconfig
    freetype
  ]
  ++ runtimeLibs;

  # The UI suite needs a display server and is exercised by CI.
  doCheck = false;

  postInstall = ''
    install -Dm644 dist/zeron.desktop $out/share/applications/zeron.desktop
    install -Dm644 dist/zeron.png $out/share/icons/hicolor/1024x1024/apps/zeron.png
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/zeron
  '';

  meta = {
    description = "Control your coding agents locally, with optional multi-device sync";
    homepage = "https://zeron.sh";
    license = lib.licenses.mit;
    mainProgram = "zeron";
    platforms = lib.platforms.linux;
  };
}
