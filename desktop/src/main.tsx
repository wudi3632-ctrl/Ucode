import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { DesktopContextMenu } from "./components/DesktopContextMenu";
import "./app/app.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
    <DesktopContextMenu />
  </StrictMode>,
);
