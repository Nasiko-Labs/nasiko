import React, { useRef, useEffect, useState } from 'react';

// Node definitions
const NODES = [
  { id: 0, label: 'AI Agent',    sub: 'LangChain', icon: '🤖', x: 60 },
  { id: 1, label: 'API Gateway', sub: 'Kong',       icon: '🔀', x: 220 },
  { id: 2, label: 'Bridge',      sub: 'stdio→HTTP', icon: '🌉', x: 380 },
  { id: 3, label: 'MCP Server',  sub: 'AI Failure', icon: '⚡', x: 540 },
];

const NODE_Y = 80;
const CANVAS_H = 200;
const CANVAS_W = 620;

export default function FlowVisualization({ nodeStates, latencyMs, totalLatency }) {
  const canvasRef = useRef(null);
  const animRef   = useRef(null);
  const particlesRef = useRef([]);

  // Spawn a new particle on the active connection
  useEffect(() => {
    const activeNode = nodeStates.findIndex(s => s === 'active');
    if (activeNode > 0) {
      const src = NODES[activeNode - 1];
      const dst = NODES[activeNode];
      particlesRef.current.push({
        x: src.x + 30,
        targetX: dst.x + 30,
        y: NODE_Y + 30,
        progress: 0,
        color: 'rgba(37,99,235,0.7)',
        id: Date.now() + Math.random(),
      });
    }
  }, [nodeStates]);

  // Canvas render loop
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');

    const render = () => {
      ctx.clearRect(0, 0, CANVAS_W, CANVAS_H);

      // Draw connector lines
      for (let i = 0; i < NODES.length - 1; i++) {
        const sx = NODES[i].x + 30;
        const dx = NODES[i + 1].x + 30;
        const bothActive = nodeStates[i] !== 'idle' && nodeStates[i + 1] !== 'idle';

        ctx.beginPath();
        ctx.setLineDash([6, 4]);
        ctx.strokeStyle = bothActive ? 'rgba(37,99,235,0.45)' : 'rgba(67,70,85,0.5)';
        ctx.lineWidth = 1.5;
        ctx.moveTo(sx, NODE_Y + 30);
        ctx.lineTo(dx, NODE_Y + 30);
        ctx.stroke();
        ctx.setLineDash([]);
      }

      // Draw nodes
      NODES.forEach((node, i) => {
        const state = nodeStates[i] || 'idle';
        const x = node.x;
        const y = NODE_Y;
        const size = 60;
        const radius = 10;

        // Shadow / glow for active
        if (state === 'active') {
          ctx.shadowColor = 'rgba(37,99,235,0.4)';
          ctx.shadowBlur  = 16;
        } else if (state === 'done') {
          ctx.shadowColor = 'rgba(34,197,94,0.2)';
          ctx.shadowBlur  = 8;
        } else {
          ctx.shadowBlur = 0;
        }

        // Node background
        const bg = state === 'active' ? '#282a2d' : state === 'done' ? '#1e2023' : '#1e2023';
        ctx.fillStyle = bg;
        ctx.beginPath();
        ctx.roundRect(x, y, size, size, radius);
        ctx.fill();
        ctx.shadowBlur = 0;

        // Border
        ctx.strokeStyle = state === 'active' ? 'rgba(37,99,235,0.55)' : state === 'done' ? 'rgba(34,197,94,0.3)' : 'rgba(67,70,85,0.4)';
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.roundRect(x, y, size, size, radius);
        ctx.stroke();

        // Icon
        ctx.font = '22px serif';
        ctx.textAlign = 'center';
        ctx.fillText(node.icon, x + 30, y + 36);

        // Status dot
        const dotColor = state === 'active' ? '#d97706' : state === 'done' ? '#22c55e' : '#434655';
        ctx.fillStyle = dotColor;
        ctx.beginPath();
        ctx.arc(x + 52, y + 8, 5, 0, Math.PI * 2);
        ctx.fill();

        // Label below
        ctx.fillStyle = '#e2e2e6';
        ctx.font = '600 11px Inter, sans-serif';
        ctx.textAlign = 'center';
        ctx.fillText(node.label, x + 30, y + 76);

        ctx.fillStyle = '#8d90a0';
        ctx.font = '400 9px Inter, sans-serif';
        ctx.fillText(node.sub, x + 30, y + 89);
      });

      // Animate particles
      particlesRef.current = particlesRef.current.filter(p => p.progress < 1);
      particlesRef.current.forEach(p => {
        p.progress = Math.min(1, p.progress + 0.03);
        const eased = p.progress < 0.5
          ? 2 * p.progress * p.progress
          : 1 - Math.pow(-2 * p.progress + 2, 2) / 2;
        p.x = (NODES[0].x + 30) + eased * (p.targetX - (NODES[0].x + 30));

        // Override with actual source
        const srcIdx = NODES.findIndex(n => n.x + 30 === p.targetX) - 1;
        const actualSrcX = srcIdx >= 0 ? NODES[srcIdx].x + 30 : NODES[0].x + 30;
        p.x = actualSrcX + eased * (p.targetX - actualSrcX);

        ctx.fillStyle = 'rgba(37,99,235,0.8)';
        ctx.shadowColor = 'rgba(37,99,235,0.5)';
        ctx.shadowBlur = 6;
        ctx.beginPath();
        ctx.arc(p.x, p.y, 4, 0, Math.PI * 2);
        ctx.fill();
        ctx.shadowBlur = 0;
      });

      // Draw total latency badge if available
      if (totalLatency > 0) {
        const badgeX = CANVAS_W / 2 - 28;
        const badgeY = 4;
        ctx.fillStyle = '#1e2023';
        ctx.strokeStyle = 'rgba(37,99,235,0.35)';
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.roundRect(badgeX, badgeY, 56, 18, 9);
        ctx.fill();
        ctx.stroke();
        ctx.fillStyle = '#b4c5ff';
        ctx.font = '600 9px "JetBrains Mono", monospace';
        ctx.textAlign = 'center';
        ctx.fillText(`${totalLatency}ms`, CANVAS_W / 2, badgeY + 13);
      }

      animRef.current = requestAnimationFrame(render);
    };

    animRef.current = requestAnimationFrame(render);
    return () => cancelAnimationFrame(animRef.current);
  }, [nodeStates, totalLatency]);

  return (
    <canvas
      ref={canvasRef}
      width={CANVAS_W}
      height={CANVAS_H}
      style={{ width: '100%', height: 'auto', display: 'block' }}
    />
  );
}
