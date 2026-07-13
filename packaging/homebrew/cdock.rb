class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.4.5"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.4.5 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.5/cdock-aarch64-macos.tar.gz"
      sha256 "b2552adbc344bf66d2005fffd278475eb2db194f8ad6b7bafd53127826ba9b24"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.5/cdock-aarch64-linux.tar.gz"
      sha256 "f5eeb77b64a71009672f82d3a0e8e4776076691b0be5c23328f928a369858e73"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.5/cdock-x86_64-linux.tar.gz"
      sha256 "292c57bf135040edfbf754e36fa8b2ffe24270a2a18ae6c882512c2d6a8f1a5b"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
