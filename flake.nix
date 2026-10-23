{
  description = "vidicon: a fast, keyboard-driven IPTV player for Linux";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs = {
    self,
    nixpkgs,
    ...
  }: let
    system = "x86_64-linux";
    pkgs = nixpkgs.legacyPackages.${system};
    # winit and the GPU backends dlopen these at runtime.
    runtimeLibs = [
      pkgs.wayland
      pkgs.libxkbcommon
      pkgs.vulkan-loader
      pkgs.libGL
      pkgs.fontconfig
      # X11, for sessions without Wayland.
      pkgs.libx11
      pkgs.libxcursor
      pkgs.libxi
      pkgs.libxrandr
    ];
  in {
    # The app to install (SPEC 12): release build without the devtools
    # feature. The libraries winit loads at runtime come first on its
    # library path; mpv and ffmpeg go last on PATH, so ones the user has
    # installed (with their config and scripts) are used, these otherwise.
    packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
      pname = "vidicon";
      version = "0.3.0";
      src = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./build.rs
          ./src
          ./ui
          ./vendor
          ./assets
          ./contrib
        ];
      };
      cargoLock.lockFile = ./Cargo.lock;
      buildNoDefaultFeatures = true;
      nativeBuildInputs = [pkgs.makeWrapper pkgs.pkg-config];
      # Linked at build time, through pkg-config.
      buildInputs = [pkgs.fontconfig];
      # The HTTP client loads the system's CA certificates even for the
      # tests' local servers; the sandbox has none.
      nativeCheckInputs = [pkgs.cacert];
      # The launcher entry and its icon, matched to the window by its
      # Wayland app id, `vidicon`. Then the Noctalia template.
      postInstall = ''
        install -Dm644 assets/vidicon.desktop -t $out/share/applications
        install -Dm644 assets/vidicon.svg -t $out/share/icons/hicolor/scalable/apps
        install -Dm644 contrib/noctalia/vidicon.toml -t $out/share/vidicon/noctalia
      '';
      # Inter comes with the package: Slint adds the files on
      # SLINT_FONT_PATH to the fonts fontconfig finds, so the user's own
      # fonts still cover the scripts Inter lacks.
      postFixup = ''
        wrapProgram $out/bin/vidicon \
          --suffix SLINT_FONT_PATH : ${pkgs.inter}/share/fonts/truetype/Inter.ttc \
          --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs} \
          --suffix PATH : ${pkgs.lib.makeBinPath [pkgs.mpv pkgs.ffmpeg]}
      '';
      meta = {
        mainProgram = "vidicon";
        license = pkgs.lib.licenses.gpl3Plus;
      };
    };

    devShells.${system}.default = pkgs.mkShell {
      packages = [
        pkgs.cargo
        pkgs.rustc
        pkgs.clippy
        pkgs.rustfmt
        pkgs.rust-analyzer
        pkgs.pkg-config
        # The demo provider's streams (tools/demo/).
        pkgs.ffmpeg
        # The website (site/).
        pkgs.nodejs
      ];
      buildInputs = runtimeLibs;
      LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
      RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
    };
  };
}
