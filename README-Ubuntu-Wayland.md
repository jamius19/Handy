# Handy on Ubuntu (GNOME Wayland)

Setup notes for running Handy on Ubuntu 26.04 with GNOME 50 on Wayland. GNOME has no
layer-shell and no clipboard data-control protocol, so a few workarounds are needed for
pasting, the overlay and the clipboard.

## 1. Build dependencies

```bash
sudo apt update
sudo apt install build-essential clang libclang-dev libevdev-dev libasound2-dev pkg-config \
  libssl-dev libvulkan-dev vulkan-tools glslc spirv-headers glslang-tools libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libgtk-layer-shell0 \
  libgtk-layer-shell-dev patchelf cmake
```

Rust (choose the standard installation; `x86_64-unknown-linux-gnu` is the correct host):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Bun:

```bash
curl -fsSL https://bun.sh/install | bash
```

Open a new terminal (or `source "$HOME/.cargo/env"`) so `cargo` and `bun` are on `PATH`.
Otherwise `bun run tauri dev` fails with `failed to run 'cargo metadata'`.

JS dependencies and the VAD model:

```bash
bun install
mkdir -p src-tauri/resources/models
curl -L -o src-tauri/resources/models/silero-vad-v6.2-F32.gguf https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/resolve/main/silero-vad-v6.2-F32.gguf
```

## 2. Build and install the deb

The bundle config creates signed updater artifacts, which needs the maintainer's private key.
Turn that off for a local build with a runtime config override (no file changes):

```bash
bun run tauri build --bundles deb --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

Install with `apt` so missing runtime dependencies (e.g. `libopenblas0`) are pulled in:

```bash
sudo apt install ./src-tauri/target/release/bundle/deb/Handy_0.11.0_amd64.deb
```

Rebuilding without bumping the version makes `apt install` skip it ("already the newest
version"). Use `reinstall` instead:

```bash
sudo apt reinstall ./src-tauri/target/release/bundle/deb/Handy_0.11.0_amd64.deb
```

## 3. Let ydotool access `/dev/uinput`

On GNOME Wayland Handy sends keys with `ydotool`, which needs the `ydotoold` daemon. Out of
the box the daemon can't open `/dev/uinput` (`root:input`, mode `660`) and exits, so pasting
fails with `ydotool failed:` and `ydotool` itself reports `failed to connect socket`.

Check the daemon log:

```bash
journalctl --user -u ydotool -n 10 --no-pager
```

`failed to open uinput device: Permission denied` confirms it.

Fix: give the logged-in session access to `/dev/uinput` only. This is narrower than joining
the `input` group, which would also allow reading all keyboard input. The rule file number
must be below 73 so the `uaccess` tag is applied.

```bash
echo 'KERNEL=="uinput", TAG+="uaccess"' | sudo tee /etc/udev/rules.d/60-uinput.rules
```

```bash
sudo udevadm control --reload && sudo udevadm trigger --name-match=uinput
```

```bash
systemctl --user reset-failed ydotool && systemctl --user restart ydotool
```

Verify (expect a `user:<you>:rw-` line and `active`):

```bash
getfacl /dev/uinput | rg "$USER" && systemctl --user is-active ydotool
```

If the ACL line is missing, log out and back in, then restart the service again.

## 4. Speed up ydotool typing (ydotool wrapper)

With the **Direct** paste method, `ydotool type` holds each key for 20 ms by default, so every
character costs about 22 ms. Handy calls plain `ydotool`, so a wrapper in `~/.local/bin`
(which is first on `PATH`) adds faster timings.

`~/.local/bin/ydotool`:

```sh
#!/bin/sh
if [ "$1" = "type" ]; then
  shift
  exec /usr/bin/ydotool type --key-delay 2 --key-hold 5 "$@"
fi
exec /usr/bin/ydotool "$@"
```

```bash
chmod +x ~/.local/bin/ydotool
```

That is about 7 ms per character. If some apps drop or repeat characters, raise
`--key-hold` to 8–10. No Handy restart is needed.

Faster still: set the paste method to **Ctrl+V** (or **Ctrl+Shift+V** for terminals) in
Handy's settings. That pastes through the clipboard with one key combo, so the time is the
same whatever the text length. The before/after delays (60 ms each) are in Debug settings
(`Ctrl+Shift+D`).

## 5. Run Handy through XWayland

As a native Wayland app on GNOME, the overlay is a normal window. GNOME's focus-stealing
prevention puts it behind the focused window and shows a "Handy is ready" notification
instead. Under XWayland the overlay shows on top as expected.

Desktop entry override (takes priority over `/usr/share/applications/Handy.desktop`):

```bash
sed 's|^Exec=handy$|Exec=env GDK_BACKEND=x11 handy|' /usr/share/applications/Handy.desktop > ~/.local/share/applications/Handy.desktop
update-desktop-database ~/.local/share/applications
```

Autostart entry (`~/.config/autostart/Handy.desktop`, created by Handy's "Launch at startup"):

```bash
sed -i 's|^Exec=/usr/bin/handy *$|Exec=env GDK_BACKEND=x11 /usr/bin/handy|' ~/.config/autostart/Handy.desktop
```

Handy rewrites the autostart file when "Launch at startup" is toggled, so reapply this
afterwards.

## 6. Handy wrapper for keyboard shortcuts

GNOME custom shortcuts run their command without a shell, so `GDK_BACKEND=x11 handy ...`
doesn't work there. A shortcut like `handy --toggle-post-process` only forwards the toggle to
a running Handy, but if Handy isn't running it starts a new one, as a native Wayland app.

A wrapper makes every plain `handy` call (shortcuts, terminal) use XWayland.

`~/.local/bin/handy`:

```sh
#!/bin/sh
exec env GDK_BACKEND=x11 /usr/bin/handy "$@"
```

```bash
chmod +x ~/.local/bin/handy
```

GNOME Shell's `PATH` starts with `~/.local/bin`, so existing shortcuts pick it up without
edits. Open terminals may need `hash -r`.

## 7. "wl-clipboard is ready" popup (code change)

GNOME has no clipboard data-control protocol, so `wl-copy` opens a tiny window to take
focus, and GNOME shows "wl-clipboard is ready". Handy used `wl-copy` whenever the session was
Wayland, even when Handy itself ran through XWayland.

The local build checks once at startup which display GTK actually opened (logged as
`GDK display backend: X11` or `Wayland`):

- `src-tauri/src/utils.rs`: `detect_gdk_backend()` and `gdk_is_wayland()`.
- `src-tauri/src/lib.rs`: calls `detect_gdk_backend()` on the main thread at startup.
- `src-tauri/src/clipboard.rs`: uses `wl-copy` only when Handy is a Wayland client;
  otherwise the clipboard plugin writes through X11, which GNOME passes on to Wayland apps.
- `src-tauri/src/overlay.rs`: skips the early overlay hide and 150 ms wait before pasting
  when Handy runs through XWayland, since the overlay can't take focus there.

Ctrl+V is still sent with `ydotool`, never `xdotool` (which under XWayland can only reach
X11 windows).
