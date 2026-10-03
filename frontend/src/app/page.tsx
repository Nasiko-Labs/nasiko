'use client';

import { useEffect } from 'react';
import Link from 'next/link';
import Navigation from '@/components/Navigation';
import Footer from '@/components/Footer';

declare global {
  interface Window {
    UnicornStudio?: {
      isInitialized?: boolean;
      init: () => void;
      destroy?: () => void;
    };
  }
}

export default function HomePage() {
  useEffect(() => {
    // Embed Unicorn Studio WebGL script for 3D interactive Sisyphus man + stars
    const embedScript = document.createElement('script');
    embedScript.type = 'text/javascript';
    embedScript.textContent = `
      !function(){
        if(!window.UnicornStudio){
          window.UnicornStudio={isInitialized:!1};
          var i=document.createElement("script");
          i.src="/scripts/unicornStudio.umd.js";
          i.onerror=function(){
            var fallback=document.createElement("script");
            fallback.src="https://cdn.jsdelivr.net/gh/hiunicornstudio/unicornstudio.js@v1.4.33/dist/unicornStudio.umd.js";
            fallback.onload=function(){
              window.UnicornStudio.isInitialized||(UnicornStudio.init(),window.UnicornStudio.isInitialized=!0)
            };
            (document.head || document.body).appendChild(fallback);
          };
          i.onload=function(){
            window.UnicornStudio.isInitialized||(UnicornStudio.init(),window.UnicornStudio.isInitialized=!0)
          };
          (document.head || document.body).appendChild(i)
        } else if (window.UnicornStudio.init) {
          try { window.UnicornStudio.init(); } catch(e){}
        }
      }();
    `;
    document.head.appendChild(embedScript);

    // If script is already cached on window, trigger init
    if (typeof window !== 'undefined' && window.UnicornStudio && typeof window.UnicornStudio.init === 'function') {
      try {
        window.UnicornStudio.init();
      } catch (e) {
        console.warn('UnicornStudio init error:', e);
      }
    }

    // Add CSS to hide branding elements and crop canvas
    const style = document.createElement('style');
    style.textContent = `
      [data-us-project] {
        position: relative !important;
        overflow: hidden !important;
      }
      
      [data-us-project] canvas {
        clip-path: inset(0 0 10% 0) !important;
      }
      
      [data-us-project] * {
        pointer-events: none !important;
      }
      [data-us-project] a[href*="unicorn"],
      [data-us-project] button[title*="unicorn"],
      [data-us-project] div[title*="Made with"],
      [data-us-project] .unicorn-brand,
      [data-us-project] [class*="brand"],
      [data-us-project] [class*="credit"],
      [data-us-project] [class*="watermark"] {
        display: none !important;
        visibility: hidden !important;
        opacity: 0 !important;
        position: absolute !important;
        left: -9999px !important;
        top: -9999px !important;
      }
    `;
    document.head.appendChild(style);

    // Function to aggressively hide branding
    const hideBranding = () => {
      const selectors = [
        '[data-us-project]',
        '[data-us-project="OMzqyUv6M3kSnv0JeAtC"]',
        '.unicorn-studio-container',
        'canvas[aria-label*="Unicorn"]',
      ];

      selectors.forEach((selector) => {
        const containers = document.querySelectorAll(selector);
        containers.forEach((container) => {
          const allElements = container.querySelectorAll('*');
          allElements.forEach((el) => {
            const text = (el.textContent || '').toLowerCase();
            const title = (el.getAttribute('title') || '').toLowerCase();
            const href = (el.getAttribute('href') || '').toLowerCase();

            if (
              text.includes('made with') ||
              text.includes('unicorn') ||
              title.includes('made with') ||
              title.includes('unicorn') ||
              href.includes('unicorn.studio')
            ) {
              const htmlEl = el as HTMLElement;
              htmlEl.style.display = 'none';
              htmlEl.style.visibility = 'hidden';
              htmlEl.style.opacity = '0';
              htmlEl.style.pointerEvents = 'none';
              htmlEl.style.position = 'absolute';
              htmlEl.style.left = '-9999px';
              htmlEl.style.top = '-9999px';
              try {
                htmlEl.remove();
              } catch {}
            }
          });
        });
      });
    };

    hideBranding();
    const interval = setInterval(hideBranding, 50);

    const t1 = setTimeout(hideBranding, 500);
    const t2 = setTimeout(hideBranding, 1000);
    const t3 = setTimeout(hideBranding, 2000);
    const t4 = setTimeout(hideBranding, 5000);
    const t5 = setTimeout(hideBranding, 10000);

    // Re-verify init after DOM finishes rendering
    const initTimer = setTimeout(() => {
      if (typeof window !== 'undefined' && window.UnicornStudio && typeof window.UnicornStudio.init === 'function') {
        try {
          window.UnicornStudio.init();
        } catch {}
      }
    }, 200);

    return () => {
      clearInterval(interval);
      clearTimeout(t1);
      clearTimeout(t2);
      clearTimeout(t3);
      clearTimeout(t4);
      clearTimeout(t5);
      clearTimeout(initTimer);
      if (embedScript.parentNode) {
        embedScript.parentNode.removeChild(embedScript);
      }
      if (style.parentNode) {
        style.parentNode.removeChild(style);
      }
    };
  }, []);

  return (
    <div className="fixed inset-0 h-screen w-screen overflow-hidden bg-black text-white flex flex-col font-mono select-none">
      {/* Background WebGL Animation - Man + Stars + Crazy Moving Shaders */}
      <div className="absolute inset-0 w-full h-full hidden lg:block z-0 pointer-events-none">
        <div
          data-us-project="OMzqyUv6M3kSnv0JeAtC"
          data-us-project-src="/embeds/OMzqyUv6M3kSnv0JeAtC.json"
          style={{ width: '100%', height: '100%', minHeight: '100vh' }}
        />
      </div>

      {/* Mobile stars background */}
      <div className="absolute inset-0 w-full h-full lg:hidden stars-bg z-0 pointer-events-none" />

      {/* Shared Top Navigation */}
      <Navigation />

      {/* Corner Frame Accents */}
      <div className="fixed top-14 left-0 w-8 h-8 lg:w-12 lg:h-12 border-t border-l border-white/30 z-20 pointer-events-none" />
      <div className="fixed top-14 right-0 w-8 h-8 lg:w-12 lg:h-12 border-t border-r border-white/30 z-20 pointer-events-none" />
      <div className="fixed bottom-9 left-0 w-8 h-8 lg:w-12 lg:h-12 border-b border-l border-white/30 z-20 pointer-events-none" />
      <div className="fixed bottom-9 right-0 w-8 h-8 lg:w-12 lg:h-12 border-b border-r border-white/30 z-20 pointer-events-none" />

      {/* Central Scrollable Workspace */}
      <main className="fixed inset-0 top-14 bottom-9 overflow-y-auto px-4 lg:px-8 z-10 scroll-container">
        <div className="max-w-7xl mx-auto min-h-full flex flex-col justify-between">
          {/* Hero Section: Right-aligned CTA content allowing the 3D moving Sisyphus man on the left to shine */}
          <div className="flex min-h-[calc(100vh-6rem)] items-center justify-end py-12 lg:py-0">
            <div className="w-full lg:w-1/2 px-6 lg:px-16 lg:pr-[10%]">
              <div className="max-w-lg relative lg:ml-auto">
                {/* Top decorative line */}
                <div className="flex items-center gap-2 mb-3 opacity-60">
                  <div className="w-8 h-px bg-white" />
                  <span className="text-white text-[10px] font-mono tracking-wider">∞</span>
                  <div className="flex-1 h-px bg-white" />
                  <span className="text-white/60 text-[9px] uppercase tracking-widest font-mono">
                    NASIKO ROUTER LAB
                  </span>
                </div>

                {/* Title with dithered accent */}
                <div className="relative">
                  <div className="hidden lg:block absolute -right-3 top-0 bottom-0 w-1 dither-pattern opacity-40" />
                  <h1
                    className="text-2xl sm:text-3xl lg:text-5xl font-bold text-white mb-3 lg:mb-4 leading-tight font-mono tracking-wider"
                    style={{ letterSpacing: '0.08em' }}
                  >
                    ROUTE WITH INTENT
                  </h1>
                </div>

                {/* Decorative dots pattern */}
                <div className="hidden lg:flex gap-1 mb-3 opacity-40">
                  {Array.from({ length: 40 }).map((_, i) => (
                    <div key={i} className="w-0.5 h-0.5 bg-white rounded-full" />
                  ))}
                </div>

                {/* Description with subtle grid pattern */}
                <div className="relative">
                  <p className="text-xs sm:text-sm lg:text-base text-gray-300 mb-5 lg:mb-6 leading-relaxed font-mono opacity-85">
                    Every user request demands different reasoning capabilities. Nasiko’s P2 Adaptive
                    Classifier analyzes task category, computes reasoning complexity, and steers queries
                    to the ideal AI model tier—delivering maximum intelligence while minimizing latency and cost.
                  </p>

                  {/* Technical corner accent - desktop only */}
                  <div
                    className="hidden lg:block absolute -left-4 top-1/2 w-3 h-3 border border-white opacity-30"
                    style={{ transform: 'translateY(-50%)' }}
                  >
                    <div
                      className="absolute top-1/2 left-1/2 w-1 h-1 bg-white"
                      style={{ transform: 'translate(-50%, -50%)' }}
                    />
                  </div>
                </div>

                {/* Buttons with technical accents */}
                <div className="flex flex-col sm:flex-row gap-3 sm:gap-4">
                  <Link
                    href="/classifier"
                    className="relative px-5 sm:px-6 py-2 sm:py-2.5 bg-white text-black font-mono text-xs sm:text-sm font-semibold border border-white hover:bg-neutral-200 transition-all text-center group active:scale-95"
                  >
                    <span className="hidden sm:block absolute -top-1 -left-1 w-2 h-2 border-t border-l border-white opacity-0 group-hover:opacity-100 transition-opacity" />
                    <span className="hidden sm:block absolute -bottom-1 -right-1 w-2 h-2 border-b border-r border-white opacity-0 group-hover:opacity-100 transition-opacity" />
                    OPEN CLASSIFIER
                  </Link>

                  <a
                    href="#how-it-works"
                    className="relative px-5 sm:px-6 py-2 sm:py-2.5 bg-transparent border border-white text-white font-mono text-xs sm:text-sm hover:bg-white hover:text-black transition-all cursor-pointer text-center active:scale-95 flex items-center justify-center"
                  >
                    HOW IT WORKS
                  </a>
                </div>

                {/* Bottom technical notation */}
                <div className="hidden lg:flex items-center gap-2 mt-6 opacity-40">
                  <span className="text-white text-[9px] font-mono">∞</span>
                  <div className="flex-1 h-px bg-white" />
                  <span className="text-white text-[9px] font-mono">P2.CLASSIFIER.PROTOCOL</span>
                </div>
              </div>
            </div>
          </div>

          {/* Section: How It Works */}
          <section id="how-it-works" className="border-t border-white/20 pt-12 pb-16 space-y-8">
            <div className="max-w-4xl space-y-2">
              <div className="flex items-center gap-2 text-white/50 text-[10px] tracking-widest uppercase">
                <span>Architecture Overview</span>
                <span>•</span>
                <span>Track P2 Specification</span>
              </div>
              <h2 className="text-xl sm:text-2xl font-bold text-white tracking-wide">
                How Request Classification &amp; Routing Works
              </h2>
              <p className="text-xs sm:text-sm text-neutral-400 leading-relaxed">
                Rather than sending every prompt to expensive frontier models, the Nasiko router inspects the incoming query before selecting an upstream model.
              </p>
            </div>

            {/* 4 Pillars Grid */}
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
              <div className="p-4 border border-white/20 bg-neutral-950/70 space-y-2 backdrop-blur-sm">
                <div className="text-[10px] text-white/50 tracking-wider">PHASE 01</div>
                <h3 className="text-sm font-bold text-white">Semantic Categorization</h3>
                <p className="text-xs text-neutral-400 leading-relaxed">
                  Buckets the query into specialized request types: Code Generation, Code Understanding,
                  Technical Design, Analytical Reasoning, Writing, or Factual Lookup.
                </p>
              </div>

              <div className="p-4 border border-white/20 bg-neutral-950/70 space-y-2 backdrop-blur-sm">
                <div className="text-[10px] text-white/50 tracking-wider">PHASE 02</div>
                <h3 className="text-sm font-bold text-white">Complexity Scoring (1–5)</h3>
                <p className="text-xs text-neutral-400 leading-relaxed">
                  Calculates cognitive depth. Single-hop questions receive Level 1, while architectural
                  refactors and mathematical proofs scale up to Level 5.
                </p>
              </div>

              <div className="p-4 border border-white/20 bg-neutral-950/70 space-y-2 backdrop-blur-sm">
                <div className="text-[10px] text-white/50 tracking-wider">PHASE 03</div>
                <h3 className="text-sm font-bold text-white">Bandit Tier Selection</h3>
                <p className="text-xs text-neutral-400 leading-relaxed">
                  Maps the query to Model Tier 1 (High Reasoning), Tier 2 (Balanced), or Tier 3 (Fast / Small)
                  using Thompson-sampling reinforcement over learned quality cells.
                </p>
              </div>

              <div className="p-4 border border-white/20 bg-neutral-950/70 space-y-2 backdrop-blur-sm">
                <div className="text-[10px] text-white/50 tracking-wider">PHASE 04</div>
                <h3 className="text-sm font-bold text-white">Safety Fallback</h3>
                <p className="text-xs text-neutral-400 leading-relaxed">
                  If confidence drops below threshold or upstream response latency spikes, the request
                  is seamlessly dispatched to a dependable safe default tier.
                </p>
              </div>
            </div>

            {/* Launch Banner */}
            <div className="p-6 border border-white/30 bg-neutral-950/80 flex flex-col sm:flex-row items-center justify-between gap-4 backdrop-blur-md">
              <div>
                <div className="text-sm font-bold text-white">Ready to test the live classifier?</div>
                <div className="text-xs text-neutral-400 mt-0.5">
                  Try standard, mixed-intent, noisy, or edge queries with instant telemetry.
                </div>
              </div>

              <Link
                href="/classifier"
                className="px-6 py-2.5 bg-white text-black font-semibold text-xs tracking-wider border border-white hover:bg-neutral-200 transition-all text-center whitespace-nowrap"
              >
                LAUNCH CLASSIFIER CONSOLE →
              </Link>
            </div>
          </section>
        </div>
      </main>

      {/* Shared Footer Telemetry Strip */}
      <Footer statusText="SYSTEM.ACTIVE" />
    </div>
  );
}
