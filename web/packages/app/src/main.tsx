import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";
import "@zeron/theme/fonts.css";
import "./styles/app.css";
import { initAppearance } from "./state/appearance";
import { RootErrorBoundary } from "./components/error-boundary";
import { router } from "./router";

initAppearance();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RootErrorBoundary>
      <RouterProvider router={router} />
    </RootErrorBoundary>
  </StrictMode>,
);
