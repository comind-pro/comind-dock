class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.4.4"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.4.4 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.4/cdock-aarch64-macos.tar.gz"
      sha256 "2b83e97de85db2201c7d899bb72a4a445331847b056445232075afc282b67910"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.4/cdock-aarch64-linux.tar.gz"
      sha256 "c02dec2cbb7fe8c65d632065e3a3bc15fd7b65e11693051db9cd419991b883cd"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.4/cdock-x86_64-linux.tar.gz"
      sha256 "cb80c3b9157627083324bdc6b62263f981aa5f3fec56fc4291ecaff5b1ceabb8"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
