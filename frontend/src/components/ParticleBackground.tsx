'use client';

import { useEffect, useRef } from 'react';
import { usePathname } from 'next/navigation';

interface DotParticle {
  x: number;
  y: number;
  radius: number;
  alpha: number;
  baseAlpha: number;
  speedY: number;
  swaySpeed: number;
  swayAmp: number;
  phase: number;
  layer: number; // 0: deep, 1: mid, 2: foreground
}

export default function ParticleBackground() {
  const pathname = usePathname();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  // On the Home page, the WebGL Unicorn Studio scene provides the man + moving stars + effects
  const isHome = pathname === '/';

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const prefersReducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    let animationFrameId: number;
    let width = 0;
    let height = 0;
    let dpr = 1;

    const setCanvasDimensions = () => {
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      width = window.innerWidth || document.documentElement.clientWidth || 1920;
      height = window.innerHeight || document.documentElement.clientHeight || 1080;
      canvas.width = Math.floor(width * dpr);
      canvas.height = Math.floor(height * dpr);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };

    setCanvasDimensions();

    // High density particle count matching screenshot (around 220-320 on desktop, 120 on mobile)
    const particleCount = Math.max(130, Math.min(320, Math.floor((width * height) / 5500)));
    const particles: DotParticle[] = [];

    for (let i = 0; i < particleCount; i++) {
      const rand = Math.random();
      let layer: number;
      let radius: number;
      let alpha: number;
      let speedY: number;

      if (rand < 0.40) {
        // Deep background layer: small, subtle, slow floating
        layer = 0;
        radius = Math.random() * 0.9 + 1.2; // 1.2 - 2.1px
        alpha = Math.random() * 0.25 + 0.40; // 0.40 - 0.65
        speedY = Math.random() * 0.2 + 0.15;
      } else if (rand < 0.82) {
        // Midground layer: crisp, clearly visible
        layer = 1;
        radius = Math.random() * 1.0 + 2.0; // 2.0 - 3.0px
        alpha = Math.random() * 0.25 + 0.65; // 0.65 - 0.90
        speedY = Math.random() * 0.3 + 0.25;
      } else {
        // Foreground layer: larger, bright white, prominent
        layer = 2;
        radius = Math.random() * 1.4 + 3.2; // 3.2 - 4.6px
        alpha = Math.random() * 0.15 + 0.85; // 0.85 - 1.00
        speedY = Math.random() * 0.4 + 0.40;
      }

      particles.push({
        x: Math.random() * width,
        y: Math.random() * height,
        radius,
        alpha,
        baseAlpha: alpha,
        speedY: prefersReducedMotion ? 0 : speedY,
        swaySpeed: Math.random() * 0.0018 + 0.0008,
        swayAmp: Math.random() * 0.8 + 0.3,
        phase: Math.random() * Math.PI * 2,
        layer,
      });
    }

    let lastTime = performance.now();

    const render = (time: number) => {
      const delta = Math.min((time - lastTime) / 16.667, 2.5);
      lastTime = time;

      ctx.clearRect(0, 0, width, height);

      // Render particles
      for (let i = 0; i < particles.length; i++) {
        const p = particles[i];

        if (!prefersReducedMotion) {
          p.y -= p.speedY * delta;
          p.x += Math.sin(time * p.swaySpeed + p.phase) * p.swayAmp * 0.12 * delta;

          // Wrap around top/bottom and sides
          if (p.y < -12) {
            p.y = height + 10;
            p.x = Math.random() * width;
          } else if (p.y > height + 12) {
            p.y = -10;
          }

          if (p.x < -12) p.x = width + 10;
          else if (p.x > width + 12) p.x = -10;
        }

        // Gentle breathing/twinkle
        const pulse = 0.88 + 0.12 * Math.sin(time * 0.0015 + p.phase);
        const currentAlpha = Math.min(1, p.baseAlpha * pulse);

        ctx.beginPath();
        ctx.arc(p.x, p.y, p.radius, 0, Math.PI * 2);
        ctx.fillStyle = `rgba(255, 255, 255, ${currentAlpha})`;
        ctx.fill();

        // Foreground particles get a delicate soft halo
        if (p.layer === 2) {
          ctx.beginPath();
          ctx.arc(p.x, p.y, p.radius * 2.2, 0, Math.PI * 2);
          ctx.fillStyle = `rgba(255, 255, 255, ${currentAlpha * 0.22})`;
          ctx.fill();
        }
      }

      animationFrameId = requestAnimationFrame(render);
    };

    animationFrameId = requestAnimationFrame(render);

    const onResize = () => {
      setCanvasDimensions();
    };

    window.addEventListener('resize', onResize);

    return () => {
      window.removeEventListener('resize', onResize);
      cancelAnimationFrame(animationFrameId);
    };
  }, [isHome]);

  if (isHome) {
    return null;
  }

  return (
    <div className="fixed inset-0 pointer-events-none z-0 overflow-hidden bg-[#030303]" aria-hidden="true">
      {/* Background Matrix Grid */}
      <div className="absolute inset-0 grid-matrix-bg opacity-35" />
      {/* Radial ambient star highlights */}
      <div className="absolute inset-0 stars-bg" />
      {/* Canvas particle stream */}
      <canvas ref={canvasRef} className="absolute inset-0 w-full h-full block" />
    </div>
  );
}
