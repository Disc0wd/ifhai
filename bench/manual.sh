#!/bin/sh
# What a careful person does by hand (same end result as ifhai), minus update-desktop-database.
set -e
H=$1; f=$H/Downloads/$2
chmod +x "$f"
mkdir -p "$H/Applications" "$H/.local/share/applications" "$H/.local/share/icons/hicolor/128x128/apps" "$H/.local/bin"
mv "$f" "$H/Applications/testapp.AppImage"
t=$(mktemp -d); cd "$t"; "$H/Applications/testapp.AppImage" --appimage-extract >/dev/null
cp squashfs-root/testapp.png "$H/.local/share/icons/hicolor/128x128/apps/testapp.png"
sed -e "s|^Exec=.*|Exec=$H/Applications/testapp.AppImage %U|" -e 's|^Icon=.*|Icon=testapp|' squashfs-root/testapp.desktop > "$H/.local/share/applications/testapp.desktop"
ln -s "$H/Applications/testapp.AppImage" "$H/.local/bin/testapp"
rm -rf "$t"
