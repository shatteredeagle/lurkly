# Maintainer: jesse
# Build from a checkout: `makepkg -si` in this directory.
pkgname=lurkly
pkgver=0.1.0
pkgrel=1
pkgdesc='Minimal Twitch window with a tab per live followed channel'
arch=(x86_64 aarch64)
license=(AGPL-3.0-or-later)
url=https://github.com/shatteredeagle/lurkly
depends=(webkit2gtk-4.1 libayatana-appindicator gtk3)
makedepends=(rust)
options=(!lto)

prepare() {
  rm -rf "$srcdir/build" && mkdir "$srcdir/build"
  tar -C "$startdir" --exclude=target -cf - dist src-tauri lurkly.svg | tar -C "$srcdir/build" -xf -
}
build() { cd "$srcdir/build/src-tauri" && cargo build --release --locked; }
package() {
  cd "$srcdir/build"
  install -Dm755 src-tauri/target/release/lurkly "$pkgdir/usr/bin/lurkly"
  install -Dm644 "$startdir/packaging/lurkly.desktop" "$pkgdir/usr/share/applications/lurkly.desktop"
  install -Dm644 "$startdir/LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 lurkly.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/lurkly.svg"
}
