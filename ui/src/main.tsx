import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { installMock } from "./mock";
import "./index.css";
import App from "./App";

installMock();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
