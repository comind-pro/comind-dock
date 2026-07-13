class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.4.3"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.4.3 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.3/cdock-aarch64-macos.tar.gz"
      sha256 "8700a6f1fa62fd891ccbb56727f48bbec372e0eb3ded0901a4916d77e6b49944"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.3/cdock-aarch64-linux.tar.gz"
      sha256 "57d0d63dbd931cb1cc25aba366007e36da17546bfe12fcbddf954e71c67f1377"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.3/cdock-x86_64-linux.tar.gz"
      sha256 "4418599141b74b87d54b3f97a7829a67766e4763e718341b33b72249b519a743"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
