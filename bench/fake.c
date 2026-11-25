/* A stand-in AppImage runtime for testing ifhai. It only ever runs code written here. */
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#ifndef APPNAME
#define APPNAME "TestApp"
#endif
#ifndef APPVER
#define APPVER "1.0"
#endif
static void w(const char *path, const char *data, size_t n) { FILE *f = fopen(path, "wb"); if (f) { fwrite(data, 1, n, f); fclose(f); } }
int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "--appimage-extract") == 0) {
        mkdir("squashfs-root", 0755);
        const char *d = "[Desktop Entry]\nType=Application\nName=" APPNAME "\nExec=AppRun %U\nIcon=testapp\nCategories=Utility;\nX-AppImage-Version=" APPVER "\n";
        w("squashfs-root/testapp.desktop", d, strlen(d));
        static const unsigned char png[] = {0x89,'P','N','G',13,10,26,10,0,0,0,13,'I','H','D','R',0,0,0,128,0,0,0,128,8,6,0,0,0};
        w("squashfs-root/testapp.png", (const char *)png, sizeof png);
        const char *run = "#!/bin/sh\necho \"AppRun of " APPNAME " " APPVER " got: $*\"\n";
        w("squashfs-root/AppRun", run, strlen(run));
        chmod("squashfs-root/AppRun", 0755);
        return 0;
    }
    printf(APPNAME " " APPVER " running directly (FUSE path), args:");
    for (int i = 1; i < argc; i++) printf(" %s", argv[i]);
    printf("\n");
    return 0;
}
