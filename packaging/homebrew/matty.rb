# Template for the Matty cask in the karyalabs/homebrew-matty tap (#203).
# scripts/homebrew-cask fills in @VERSION@ and @SHA256@ from a release's
# SHA256SUMS; the release workflow opens the bump pull request on the tap.
cask "matty" do
  version "@VERSION@"
  sha256 "@SHA256@"

  url "https://github.com/karyalabs/matty/releases/download/v#{version}/Matty-#{version}.dmg"
  name "Matty"
  desc "Unofficial native desktop client for Mattermost"
  homepage "https://github.com/karyalabs/matty"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The in-app updater is the one update path (PLAN §10), so brew leaves an
  # installed copy alone unless asked with --greedy.
  auto_updates true
  depends_on macos: ">= :ventura"

  app "Matty.app"

  zap trash: [
    "~/Library/Application Support/Matty",
    "~/Library/Caches/Matty",
    "~/Library/Logs/Matty",
    "~/Library/Preferences/dev.arya.matty.plist",
    "~/Library/Saved Application State/dev.arya.matty.savedState",
  ]
end
