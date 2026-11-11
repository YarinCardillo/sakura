cask "sakura" do
  version "0.1.1"
  sha256 "e5c950e729504b997b28af06f90fde0390d1218a6067dfc480c744d586205520"

  url "https://github.com/YarinCardillo/sakura/releases/download/v#{version}/Sakura-#{version}.zip"
  name "Sakura"
  desc "Desk for watching and steering coding agents in confined workspaces"
  homepage "https://github.com/YarinCardillo/sakura"

  depends_on macos: :big_sur
  depends_on arch: :arm64

  app "Sakura.app"
end
