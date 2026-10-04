cask "backbeat" do
  version "1.0.0"
  sha256 "3aa55a24b95abb503a8929084380f915089b35945f9d3220b30e114fd3945967"

  url "https://github.com/zkldi/backbeat/releases/download/v#{version}/Backbeat_#{version}_universal.dmg"
  name "Backbeat"
  desc "Manage and install rhythm game charts"
  homepage "https://backbeat.ac/"

  depends_on macos: :big_sur

  app "Backbeat.app"

  caveats <<~EOS
    Backbeat is not signed with an Apple Developer ID. If macOS blocks the
    first launch, try opening Backbeat and then allow it under:

      System Settings → Privacy & Security → Open Anyway
  EOS
end
