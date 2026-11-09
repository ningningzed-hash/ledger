/** 助手头部的副标题:模型名 + 本地/远程 */
export function isLocalUrl(url: string): boolean {
  try {
    const h = new URL(url).hostname.replace(/^\[|\]$/g, "");
    return h === "localhost" || h === "::1" || h === "0.0.0.0" || /^127\./.test(h);
  } catch {
    return false;
  }
}

export function modelLabel(url: string, model: string): string {
  return `${model} · ${isLocalUrl(url) ? "本地" : "远程"}`;
}
