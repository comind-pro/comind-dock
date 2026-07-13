class Cdock < Formula
  desc "Terminal-native runtime and multiplexer for AI coding agents"
  homepage "https://github.com/comind-pro/comind-dock"
  version "0.4.6"
  license "MIT"

  # ponytail: no macOS x86_64 asset in v0.4.6 — add an on_intel block when the release ships one
  on_macos do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.6/cdock-aarch64-macos.tar.gz"
      sha256 "da9d4b1c02b39006c7a81f8ea2eb7d68a5852d73ea3aa9e87259047c579d56a7"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.6/cdock-aarch64-linux.tar.gz"
      sha256 "81272044153c8a12433494c480c8dd894e4e9d24e061461b508559f3d2600598"
    end
    on_intel do
      url "https://github.com/comind-pro/comind-dock/releases/download/v0.4.6/cdock-x86_64-linux.tar.gz"
      sha256 "e4027a43a54bf831d2d67d3c2ce027cd60fe84003a5e133c8feddde58fbca6f1"
    end
  end

  def install
    bin.install "cdock"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/cdock --version")
  end
end
