# Installing

Each [release](https://github.com/zanmato/blanco/releases) has builds for
macOS on Apple silicon, Windows on x86_64 and Linux on x86_64.

## macOS

Install it with [Homebrew](https://brew.sh):

```sh
brew install --cask zanmato/tap/blanco
```

The app isn't notarized yet, and the cask clears its quarantine flag for you.
To install it by hand instead, download `Blanco.app`, unpack it, move it to
Applications and clear the flag before the first start:

```sh
xattr -r -d com.apple.quarantine Blanco.app
```

## Windows

Download `blanco.exe`. It is a single executable with no installer.

## Linux

Install the `.deb` package:

```sh
sudo apt install ./blanco_<version>_amd64.deb
```

Or download the plain binary and put it somewhere on your `PATH`.

## Nix

The repository is a flake for x86_64 and aarch64 Linux. It builds Blanco from
source, so the first install compiles for a while.

```sh
nix run github:zanmato/blanco                  # try it
nix profile install github:zanmato/blanco      # install it
```

On NixOS, add the flake as an input and its overlay to your configuration,
which builds Blanco against your system's own libraries:

```nix
{
  inputs.blanco.url = "github:zanmato/blanco";

  outputs = { nixpkgs, blanco, ... }: {
    nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
      modules = [
        { nixpkgs.overlays = [ blanco.overlays.default ]; }
        ({ pkgs, ... }: { environment.systemPackages = [ pkgs.blanco ]; })
      ];
    };
  };
}
```

Blanco draws with Vulkan. NixOS provides the driver, but with Nix on another
distribution a Nix-built program cannot load the system's driver, so start it
through [nixGL](https://github.com/nix-community/nixGL).

`nix develop` opens a shell with the toolchain and libraries for working on
Blanco.

## Updates

Blanco checks for a new release once an hour. When one is out, an **Update**
button appears in the title bar. Click it to download the release, then
**Restart** to switch to it. Downloads are checked against the release's
signed checksums before they are used.

The `.deb` and Nix install into a folder Blanco cannot write to, so there the
button opens the release page instead. Upgrade a Nix install with
`nix profile upgrade blanco`, or by updating the flake input.

To stop checking, turn off **Check for Updates** in Settings.
