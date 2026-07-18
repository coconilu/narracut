import { open } from "@tauri-apps/plugin-dialog";

async function pickSingleFile(
  title: string,
  name: string,
  extensions: readonly string[],
): Promise<string | undefined> {
  const selection = await open({
    title,
    multiple: false,
    directory: false,
    filters: [{ name, extensions: [...extensions] }],
  });
  return typeof selection === "string" ? selection : undefined;
}

export function pickAudioFile(): Promise<string | undefined> {
  return pickSingleFile("选择口播 WAV 音频", "WAV 音频", ["wav"]);
}

export function pickCaptionsFile(): Promise<string | undefined> {
  return pickSingleFile("选择 SRT 字幕", "SRT 字幕", ["srt"]);
}
