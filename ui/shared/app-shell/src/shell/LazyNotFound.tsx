import { lazy } from "react";
export const LazyNotFound = lazy(() => import("./NotFound").then((m) => ({ default: m.NotFound })));
