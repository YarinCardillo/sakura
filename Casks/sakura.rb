cask "sakura" do
  version "0.1.2"
  sha256 "8dd96e4c18e04289b6d92a1147355d91a4fca84c7025ed089817380e6a3dfd9c"

  url "https://github.com/YarinCardillo/sakura/releases/download/v#{version}/Sakura-#{version}.zip"
  name "Sakura"
  desc "Desk for watching and steering coding agents in confined workspaces"
  homepage "https://github.com/YarinCardillo/sakura"

  depends_on macos: :big_sur
  depends_on arch: :arm64

  app "Sakura.app"
end
