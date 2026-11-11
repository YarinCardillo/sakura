cask "sakura" do
  version "0.1.0"
  sha256 "6651bd62bf9bb607e54f8e16c9ef90ab216874ffd9d50f811cb537d8570ffb8c"

  url "https://github.com/YarinCardillo/sakura/releases/download/v#{version}/Sakura-#{version}.zip"
  name "Sakura"
  desc "Desk for watching and steering coding agents in confined workspaces"
  homepage "https://github.com/YarinCardillo/sakura"

  depends_on macos: :big_sur
  depends_on arch: :arm64

  app "Sakura.app"
end
