# Homebrew formula for `fl` — drop this into the tap repo
# `Antoinegtir/homebrew-flutter-cli` under `Formula/fl.rb`, then users
# install with:
#
#   brew tap antoinegtir/flutter-cli
#   brew install fl
#
# The `version` and four `sha256` lines below are rewritten in place by
# the `homebrew-update` job in `.github/workflows/release.yml` every
# time a new `vX.Y.Z` tag is pushed, so you should rarely need to edit
# this by hand. The placeholder SHAs let `brew audit` / `brew style`
# parse the file cleanly until the first real release lands.
#
# If you're publishing your first release manually:
#   1. Push a `v0.1.0` tag to your main repo (this generates the
#      tarballs at https://github.com/<repo>/releases).
#   2. Run `shasum -a 256 fl-0.1.0-<target>.tar.gz` for each tarball.
#   3. Paste the digests below and update the URLs to point at your
#      GitHub repo.

class Fl < Formula
  desc "Modern Flutter CLI with seamless USB→WiFi hot reload"
  # TODO: replace with your repo URL.
  homepage "https://github.com/Antoinegtir/flutter-cli"
  version "0.1.0"
  license "MIT"

  # GitHub Releases hosts a separate tarball per (OS, arch) pair. We
  # pick the right one at install time via the on_* blocks so a single
  # `brew install fl` works on every supported platform.
  on_macos do
    on_arm do
      url "https://github.com/Antoinegtir/flutter-cli/releases/download/v#{version}/fl-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/Antoinegtir/flutter-cli/releases/download/v#{version}/fl-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Antoinegtir/flutter-cli/releases/download/v#{version}/fl-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/Antoinegtir/flutter-cli/releases/download/v#{version}/fl-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    # Each tarball ships a single `fl` binary at its root (Homebrew
    # strips the top-level `fl-X.Y.Z-<target>/` directory for us).
    bin.install "fl"
  end

  # `brew livecheck fl` will report the latest GitHub Release tag, so
  # contributors can `brew outdated fl` and verify a bump is needed.
  livecheck do
    url :stable
    strategy :github_latest
  end

  test do
    # Smoke test: `fl --version` should print the same version Homebrew
    # tracks. Catches the case where the wrong-arch tarball was somehow
    # downloaded (the binary would refuse to run, this test would fail).
    assert_match version.to_s, shell_output("#{bin}/fl --version")
  end
end
