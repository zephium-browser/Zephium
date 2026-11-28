cask "zephium" do
  version "1.0.5"
  sha256 "11c35eace71e21af87ccdbc3049f96d028850faf1c2013bcfc36d4b9b7d4bcdc"

  url "https://github.com/zephium-browser/Zephium/releases/download/v#{version}/Zephium-macOS-arm64.dmg",
      verified: "github.com/zephium-browser/Zephium/releases/download/"
  name "Zephium"
  desc "Privacy-first browser with a built-in AI workspace"
  homepage "https://zephium.app"

  depends_on arch: :arm64

  app "Zephium.app"

  zap trash: [
    "~/Library/Application Support/app.zephium",
    "~/Library/Caches/app.zephium",
    "~/Library/Containers/app.zephium",
    "~/Library/Preferences/app.zephium.plist",
    "~/Library/Saved Application State/app.zephium.savedState",
    "~/Library/WebKit/app.zephium",
  ]
end
