#!/bin/bash
# Download Termux's proot (bionic) and its shared deps, verify the pinned
# debs, and repackage them as jniLibs/<abi>/lib*.so — nativeLibraryDir is the
# only place an Android app may exec from (docs/android.md § Runtime contract).
#
#   scripts/android/fetch-proot.sh [out_dir]      # default target/android-runtime
#
# Needs curl, sha256sum, ar, tar with xz, unzip, patch, make, patchelf
# ($PATCHELF or on PATH) and the NDK ($ANDROID_NDK_HOME) for the x86_64 build.
#
# x86_64 proot is rebuilt from the same pinned source with one patch: Android's
# x86_64 app seccomp policy has no fork/vfork (bionic only uses clone), musl
# calls them, and stock proot turns the SIGSYS into ENOSYS — so every shell
# pipeline in the guest fails on emulators. arm64 has no fork syscall at all
# (musl uses clone there), so it ships Termux's binary untouched.
#
# Termux keeps only the newest build of a package in its pool, so a pinned deb
# can 404 after an upstream bump: the error prints the current version/sha256
# from the index — review, then update the pins below.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/android-runtime}"
CACHE="$OUT/cache/termux"
REPO="${TERMUX_REPO:-https://packages.termux.dev/apt/termux-main}"
PROOT_VERSION=5.1.107.95

# name version arch sha256 — from dists/stable/main/binary-<arch>/Packages.
PINS=(
  "proot $PROOT_VERSION aarch64 0a1b3d0f6ef76436c5ed924cd8e8f5a6b7186e99e1650eb2d9bc734e218a74cb"
  "libtalloc 2.4.3 aarch64 ac81ad623d74c209718b9f3acb2dd702cc8a88c431e820d212229910b4db29da"
  "libandroid-shmem 0.7 aarch64 0da3a24d558b93c92bcf8d611e0826a99ff96e396b148e6cdf33b47c47c57ff6"
  "proot $PROOT_VERSION x86_64 f63ce9bd0d38715eae0163a3772f3395913587444c7ce7232091c6d359afe3c3"
  "libtalloc 2.4.3 x86_64 7ca2eaae2e53b28228a01301bc410b62845403d6317c25b8e0a7f40681de0628"
  "libandroid-shmem 0.7 x86_64 ffa9e4c87467b158b148d0ff92dda796aa038276c2075af3269cdcdb06f25797"
)
# The source Termux built that deb from (termux-packages packages/proot/build.sh).
PROOT_SRC_URL="https://github.com/termux/proot/archive/v$PROOT_VERSION.zip"
PROOT_SRC_SHA256=dbb50381c2f0b5c342bdf3d3467d80c21d2a4677d9dadd14159fa3b32f11b319

die() { echo "error: $*" >&2; exit 1; }
for tool in curl sha256sum ar tar xz unzip patch make; do
  command -v "$tool" >/dev/null || die "$tool not found"
done
PATCHELF="${PATCHELF:-$(command -v patchelf || true)}"
[[ -x "$PATCHELF" ]] || die "patchelf not found — install it (pacman -S patchelf / apt install patchelf / pip install patchelf) or set PATCHELF=/path/to/patchelf"
NDK_BIN="${ANDROID_NDK_HOME:-}/toolchains/llvm/prebuilt/$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64/bin"
[[ -x "$NDK_BIN/x86_64-linux-android24-clang" ]] \
  || die "Android NDK not found — set ANDROID_NDK_HOME (NDK 27, needed to build the x86_64 proot)"

abi_for() { case "$1" in aarch64) echo arm64-v8a ;; x86_64) echo x86_64 ;; esac; }
pool_path() { # libtalloc → pool/main/libt/libtalloc, proot → pool/main/p/proot
  local name="$1" prefix="${1:0:1}"
  [[ "$name" == lib* ]] && prefix="${name:0:4}"
  echo "pool/main/$prefix/$name"
}

current_pin() { # what the live index says, to make a 404 actionable
  curl -fsSL "$REPO/dists/stable/main/binary-$2/Packages" 2>/dev/null \
    | awk -v p="$1" 'BEGIN{RS=""} $0 ~ "(^|\n)Package: "p"\n"' \
    | grep -E '^(Version|SHA256):' | tr '\n' ' ' || true
}

fetch() { # url sha dest — cached, verified download
  local url="$1" sha="$2" dest="$3"
  mkdir -p "$(dirname "$dest")"
  if [[ -f "$dest" ]] && echo "$sha  $dest" | sha256sum -c --status; then return; fi
  echo "fetch $(basename "$dest")" >&2
  curl -fsSL --retry 3 -o "$dest.part" "$url" || { rm -f "$dest.part"; return 1; }
  if ! echo "$sha  $dest.part" | sha256sum -c --status; then
    rm -f "$dest.part"
    die "sha256 mismatch for $(basename "$dest") (expected $sha)"
  fi
  mv "$dest.part" "$dest"
}

fetch_deb() { # name version arch sha → cached, verified path
  local name="$1" version="$2" arch="$3" sha="$4"
  local file="${name}_${version}_${arch}.deb"
  fetch "$REPO/$(pool_path "$name")/$file" "$sha" "$CACHE/$file" \
    || die "download of $file failed; the index currently has $name: $(current_pin "$name" "$arch")— update PINS in $0"
  echo "$CACHE/$file"
}

extract_deb() { # deb → dir with the package's data/ tree
  local deb="$1" dir="$2" member
  rm -rf "$dir"; mkdir -p "$dir"
  member="$(ar t "$deb" | grep '^data\.tar')" || die "$deb has no data.tar member"
  (cd "$dir" && ar x "$deb" "$member" && tar -xf "$member" && rm -f "$member")
}

# Rewrites fork/vfork, which the x86_64 app seccomp filter rejects, to clone.
PROOT_FORK_PATCH='--- a/src/tracee/seccomp.c
+++ b/src/tracee/seccomp.c
@@ -7,6 +7,7 @@
 #include <linux/net.h> /* SYS_SENDMMSG */
 #include <assert.h>    /* assert(3), */
 #include <time.h>      /* time(2), */
+#include <sched.h>     /* CLONE_*, */

 #include "extension/extension.h"
 #include "cli/note.h"
@@ -626,6 +627,22 @@
 		break;
 	}

+#if defined(ARCH_X86_64)
+	case PR_fork:
+	case PR_vfork:
+		/* Android'"'"'s x86_64 app seccomp policy has no fork/vfork (bionic
+		 * only uses clone) but musl calls them.  Only the flags and
+		 * stack arguments change: the child doesn'"'"'t get the parent'"'"'s
+		 * register restore, and musl'"'"'s fork/vfork keep nothing live in
+		 * rdi/rsi across the syscall.  */
+		set_sysnum(tracee, PR_clone);
+		poke_reg(tracee, SYSARG_1, sysnum == PR_vfork
+			 ? (CLONE_VM | CLONE_VFORK | SIGCHLD) : SIGCHLD);
+		poke_reg(tracee, SYSARG_2, 0);
+		restart_syscall_after_seccomp(tracee);
+		break;
+#endif
+
 	case PR_set_robust_list:
 	default:
 		/* Set errno to -ENOSYS */
'

build_proot_x86_64() { # work_dir usr_rel dest_dir → dest/libproot.so from patched source
  local work="$1" usr="$2" dest="$3" src="$1/proot-src"
  local zip="$OUT/cache/proot-src-$PROOT_VERSION.zip"
  fetch "$PROOT_SRC_URL" "$PROOT_SRC_SHA256" "$zip" || die "download of $PROOT_SRC_URL failed"
  rm -rf "$src"; mkdir -p "$src"
  unzip -q "$zip" -d "$src"
  src="$src/proot-$PROOT_VERSION"
  echo "$PROOT_FORK_PATCH" | patch -s -p1 -d "$src"
  # Link against the SONAME-patched libs so NEEDED is already libtalloc.so.
  CFLAGS="-Wno-error=implicit-function-declaration -fstack-protector-strong" \
  CPPFLAGS="-DARG_MAX=131072 -DVERSION=\\\"$PROOT_VERSION\\\" -I$work/libtalloc/$usr/include -I$work/libandroid-shmem/$usr/include" \
  LDFLAGS="-L$dest" \
    make -s -C "$src/src" -j"$(nproc 2>/dev/null || echo 4)" proot \
      PROOT_WITH_LIBANDROID_SHMEM=true PROOT_UNBUNDLE_LOADER=/nonexistent \
      CC="$NDK_BIN/x86_64-linux-android24-clang" STRIP="$NDK_BIN/llvm-strip" \
      OBJCOPY="$NDK_BIN/llvm-objcopy" OBJDUMP="$NDK_BIN/llvm-objdump" >"$work/build.log" 2>&1 \
    || { tail -30 "$work/build.log" >&2; die "proot build failed (full log: $work/build.log)"; }
  install -m 0755 "$src/src/proot" "$dest/libproot.so"
  "$NDK_BIN/llvm-strip" "$dest/libproot.so"
}

for arch in aarch64 x86_64; do
  abi="$(abi_for "$arch")"
  work="$OUT/cache/proot-work/$arch"
  for pin in "${PINS[@]}"; do
    read -r name version parch sha <<<"$pin"
    [[ "$parch" == "$arch" ]] || continue
    extract_deb "$(fetch_deb "$name" "$version" "$parch" "$sha")" "$work/$name"
  done

  usr="data/data/com.termux/files/usr"
  dest="$OUT/jniLibs/$abi"
  mkdir -p "$dest"
  install -m 0755 "$work/proot/$usr/libexec/proot/loader" "$dest/libproot-loader.so"
  if [[ -f "$work/proot/$usr/libexec/proot/loader32" ]]; then
    install -m 0755 "$work/proot/$usr/libexec/proot/loader32" "$dest/libproot-loader32.so"
  fi
  install -m 0755 "$(readlink -f "$work/libtalloc/$usr/lib/libtalloc.so.2")" "$dest/libtalloc.so"
  install -m 0755 "$work/libandroid-shmem/$usr/lib/libandroid-shmem.so" "$dest/libandroid-shmem.so"

  # The package manager only extracts lib*.so, so every NEEDED/SONAME must use
  # that shape, and Termux's RUNPATH (/data/data/com.termux/…) must go.
  "$PATCHELF" --set-soname libtalloc.so "$dest/libtalloc.so"
  "$PATCHELF" --remove-rpath "$dest/libtalloc.so"
  "$PATCHELF" --remove-rpath "$dest/libandroid-shmem.so"

  if [[ "$arch" == x86_64 ]]; then
    build_proot_x86_64 "$work" "$usr" "$dest"
  else
    install -m 0755 "$work/proot/$usr/bin/proot" "$dest/libproot.so"
    "$PATCHELF" --replace-needed libtalloc.so.2 libtalloc.so "$dest/libproot.so"
  fi
  "$PATCHELF" --remove-rpath "$dest/libproot.so"
  "$PATCHELF" --print-needed "$dest/libproot.so" | grep -qx libtalloc.so \
    || die "libproot.so ($abi) doesn't link libtalloc.so"
  echo "proot ($abi): $dest"
done
