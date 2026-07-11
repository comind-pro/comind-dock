class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.4.0"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.4.0 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.0/cdock-aarch64-macos.tar.gz"
      sha256 "6c3170577e29bf42ff91c327f1566bfae398ab781f59643beef5c4c65d9cc8f4"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.0/cdock-aarch64-linux.tar.gz"
      sha256 "cf7ca5c6bdbf4d6147253c4ab15b891fd16ad106e7f33967a00e5064ab9ff642"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.0/cdock-x86_64-linux.tar.gz"
      sha256 "16ee4ea2ad80672a67d4f850bc268c97d8f75588d6339217104a1f46339430c2"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
