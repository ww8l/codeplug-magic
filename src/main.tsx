import React from "react";
import ReactDOM from "react-dom/client";
import { Toaster } from "sonner";
import App from "./App";
import { ThemeProvider, useTheme } from "./theme";
import "./index.css";

function ThemedToaster() {
  const { resolved } = useTheme();
  return <Toaster theme={resolved} position="bottom-right" richColors closeButton />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <App />
      <ThemedToaster />
    </ThemeProvider>
  </React.StrictMode>,
);
