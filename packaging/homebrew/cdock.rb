class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.4.1"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.4.1 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.1/cdock-aarch64-macos.tar.gz"
      sha256 "51e15cdffca94e8f28cd29bb71067bf6e908e311dcecb3ae0d00be33c10296db"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.1/cdock-aarch64-linux.tar.gz"
      sha256 "bf1ec36281f76c016d921dc9b0151644a253a375494229431d8aa5d96b0c5782"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.1/cdock-x86_64-linux.tar.gz"
      sha256 "28489c6231e22d85856277662070478eb14800a108067a3f6070823ffa425de9"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
