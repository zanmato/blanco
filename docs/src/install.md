# Installing

Each [release](https://github.com/zanmato/blanco/releases) has builds for
macOS on Apple silicon, Windows on x86_64 and Linux on x86_64.

## macOS

Download `Blanco.app`, unpack it and move it to Applications. The app isn't
notarized yet, so clear the quarantine flag before the first start:

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

## Updates

Blanco checks for a new release once an hour. When one is out, an **Update**
button appears in the title bar. Click it to download the release, then
**Restart** to switch to it. Downloads are checked against the release's
signed checksums before they are used.

The `.deb` installs into a folder Blanco cannot write to, so there the button
opens the release page instead.

To stop checking, turn off **Check for Updates** in Settings.
