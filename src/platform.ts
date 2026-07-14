import { useSyncExternalStore } from "react";
import { api } from "./api";

// macOS 限定 UI（アカウント切替・環境診断・Finder/cmux/Ghostty）の出し分け用。
// 取得完了までは従来どおりの macOS 挙動に倒す（既存ユーザーは全員 macOS のため
// ちらつきが出ない側を初期値にする）
let current = "macos";
const listeners = new Set<() => void>();

api
  .getPlatform()
  .then((p) => {
    current = p;
    listeners.forEach((l) => l());
  })
  .catch(() => {
    // 取得失敗時は初期値のまま（機能を隠しすぎない）
  });

export function usePlatform(): string {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => current
  );
}

/** macOS 限定機能（アカウント切替・環境診断・外部アプリ起動）を表示してよいか */
export function useIsMac(): boolean {
  return usePlatform() === "macos";
}
