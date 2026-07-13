class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.4.2"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.4.2 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.2/cdock-aarch64-macos.tar.gz"
      sha256 "867e236c216086bd49687a5ac7e2fcd2fa2b64cd20ae4649f01246bb0ef72190"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.2/cdock-aarch64-linux.tar.gz"
      sha256 "716bed204d651e470607840bac1a657807400a2b5725d23e887de2e189233157"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.2/cdock-x86_64-linux.tar.gz"
      sha256 "fc22c17ddd408e2191243ac633a789581b1d239eb483aaa86921d9a0e74c7ce3"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
