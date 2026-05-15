# Ubuntu X11 Remote Desktop Testing

This project currently supports Linux desktop capture through X11 tools. Ubuntu 26.04 uses GNOME Wayland by default, so remote desktop capture will fail with:

```text
Linux capture requires maim or ImageMagick import on X11; Wayland needs a portal backend
```

Use an X11 session for Linux remote desktop testing until a Wayland portal/PipeWire backend is implemented.

## Install the Required Packages

Install an X11 desktop session, a display manager that can start it, and the capture/input tools used by the client:

```bash
sudo apt update
sudo apt install -y xfce4 lightdm x11-xserver-utils maim imagemagick xdotool
```

If the installer asks for the default display manager, select:

```text
lightdm
```

If no prompt appears, run:

```bash
sudo dpkg-reconfigure lightdm
```

Select `lightdm`, then reboot:

```bash
sudo reboot
```

## Log In With X11

After rebooting:

1. Select your user on the login screen.
2. Open the session selector near the username or in a screen corner.
3. Select `Xfce Session`.
4. Enter your password and log in.

Confirm the session type:

```bash
echo $XDG_SESSION_TYPE
```

The expected output is:

```text
x11
```

## Verify Capture Tools

Before starting the rust-desk-light client, verify that X11 screenshot capture works:

```bash
maim /tmp/rdl-test.jpg
file /tmp/rdl-test.jpg
```

The `file` command should report a JPEG image. If this works, start the client and test remote desktop from the admin application.

The client uses `xrandr` from `x11-xserver-utils` to list screens, `maim` or ImageMagick `import` to capture frames, and `xdotool` for mouse input.

## Current Wayland Limitation

Wayland capture requires an xdg-desktop-portal ScreenCast session and PipeWire frame reading. This is not implemented yet. The current Linux backend only tries:

- `maim` on X11
- ImageMagick `import` on X11

Mouse input on Linux uses `xdotool`, which also targets X11.
