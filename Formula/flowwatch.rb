class Flowwatch < Formula
  desc "Lightweight, privacy-focused macOS per-application traffic monitor"
  homepage "https://github.com/JunieXD/FlowWatch"
  license "MIT"
  depends_on macos: :ventura

  if Hardware::CPU.arm?
    url "https://github.com/JunieXD/FlowWatch/releases/download/v0.1.4/flowwatch-aarch64-apple-darwin.tar.gz"
    sha256 "e3a5227241ef18cc486e0e3d6fa6fb670a41071b909b566a8f1715f86e2aaa9b"
  else
    url "https://github.com/JunieXD/FlowWatch/releases/download/v0.1.4/flowwatch-x86_64-apple-darwin.tar.gz"
    sha256 "f216dbf23da293d4e09033944747bdac3979b0ba4773f1bf6e8a7491db03039b"
  end

  def install
    bin.install "flowwatch"
  end

  def caveats
    <<~EOS
      安装或升级后，请运行下面的命令安装并启动登录自启服务：
        flowwatch install

      查看采集状态：
        flowwatch doctor
        flowwatch status
    EOS
  end

  test do
    assert_match "flowwatch #{version}", shell_output("#{bin}/flowwatch --version")
    assert_match "macOS 应用流量统计工具", shell_output("#{bin}/flowwatch --help")
  end
end
