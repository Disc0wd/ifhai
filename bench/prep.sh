#!/bin/sh
rm -rf "$1"/* "$1"/.local "$1"/.[a-z]*; mkdir -p "$1/Downloads"; cp "$2" "$1/Downloads/$3"; chmod 644 "$1/Downloads/$3"
