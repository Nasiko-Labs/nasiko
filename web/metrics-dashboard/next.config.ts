import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // When served behind Kong at /app/metrics/, set basePath: "/app/metrics" and deploy with strip_path.
  output: "standalone",
};

export default nextConfig;
