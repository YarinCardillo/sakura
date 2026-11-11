cask "sakura" do
  version "0.1.0"
  sha256 "6cebd58e12c1d779375cbe70149d830791f568a2e700ebdbc8e176a8b2791c5d"

  url "https://github.com/YarinCardillo/sakura/releases/download/v#{version}/Sakura-#{version}.zip"
  name "Sakura"
  desc "Desk for watching and steering coding agents in confined workspaces"
  homepage "https://github.com/YarinCardillo/sakura"

  depends_on macos: :big_sur
  depends_on arch: :arm64

  app "Sakura.app"
end
