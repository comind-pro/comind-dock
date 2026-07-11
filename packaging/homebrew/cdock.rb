class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.3.0"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.3.0 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.3.0/cdock-aarch64-macos.tar.gz"
      sha256 "9d3f0ed6c2c09b2aa7ad64efa1045175de88146625ece0bfd0ee63c915c47d6e"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.3.0/cdock-aarch64-linux.tar.gz"
      sha256 "9a4f531412219cc7df776e3c0d630d3ee242b2bddb119cf91c4d17a0776e7de6"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.3.0/cdock-x86_64-linux.tar.gz"
      sha256 "a25e88dc74c523cea99b6d4af4b577d2133cdb27fa5e007c066835c4c21f49a3"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
