"use client";

import { createContext, useContext } from "react";

const StaticChartPreviewContext = createContext(false);

export function useStaticChartPreview() {
  return useContext(StaticChartPreviewContext);
}
