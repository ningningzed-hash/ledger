export const DEFAULT_CHAT_WIDTH = 560;
export const CHAT_WIDTH_KEY = "ledger.chat.width";

export function clampChatWidth(width: number, viewport: number): number {
  const max = Math.max(0, viewport);
  return Math.min(max, Math.max(Math.min(360, max), Number.isFinite(width) ? width : DEFAULT_CHAT_WIDTH));
}

export function readChatWidth(key = CHAT_WIDTH_KEY): number {
  try {
    const saved = Number(localStorage.getItem(key));
    return Number.isFinite(saved) && saved >= 360 ? saved : DEFAULT_CHAT_WIDTH;
  } catch { return DEFAULT_CHAT_WIDTH; }
}
