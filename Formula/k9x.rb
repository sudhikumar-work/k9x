class K9x < Formula
  desc "Event-driven Kubernetes TUI + agent CLI — ultra-fast, single binary"
  homepage "https://github.com/sudhikumar-work/k9x"
  version "0.2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/sudhikumar-work/k9x/releases/download/v#{version}/k9x-#{version}-darwin-arm64.tar.gz"
      sha256 "REPLACE_WITH_DARWIN_ARM64_SHA256"
    else
      url "https://github.com/sudhikumar-work/k9x/releases/download/v#{version}/k9x-#{version}-darwin-amd64.tar.gz"
      sha256 "REPLACE_WITH_DARWIN_AMD64_SHA256"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/sudhikumar-work/k9x/releases/download/v#{version}/k9x-#{version}-linux-arm64.tar.gz"
      sha256 "REPLACE_WITH_LINUX_ARM64_SHA256"
    else
      url "https://github.com/sudhikumar-work/k9x/releases/download/v#{version}/k9x-#{version}-linux-amd64.tar.gz"
      sha256 "REPLACE_WITH_LINUX_AMD64_SHA256"
    end
  end

  def install
    bin.install "k9x"
    # Install shell completions if present
    if File.exist?("contrib/completions/k9x.bash")
      bash_completion.install "contrib/completions/k9x.bash" => "k9x"
    end
    if File.exist?("contrib/completions/k9x.zsh")
      zsh_completion.install "contrib/completions/k9x.zsh" => "_k9x"
    end
    if File.exist?("contrib/completions/k9x.fish")
      fish_completion.install "contrib/completions/k9x.fish"
    end
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/k9x --version")
  end
end
