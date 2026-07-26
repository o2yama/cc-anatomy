import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { TrayPanel } from "./TrayPanel";

// tray-panel ウィンドウ（tauri.conf.json）は同じビルドを ?panel=tray 付きで読み込む。
// 別ルーターを組むほどの規模ではないので、起動時に1回だけ出し分ける
const isTrayPanel = new URLSearchParams(window.location.search).get("panel") === "tray";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isTrayPanel ? <TrayPanel /> : <App />}</React.StrictMode>,
);
