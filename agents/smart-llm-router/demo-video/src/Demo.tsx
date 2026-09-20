import React from "react";
import {
  AbsoluteFill,
  Img,
  Sequence,
  interpolate,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
  Easing,
} from "remotion";
import { loadFont as loadSyne } from "@remotion/google-fonts/Syne";
import { loadFont as loadDM } from "@remotion/google-fonts/DMSans";
import { scene } from "./timing";

const { fontFamily: syne } = loadSyne();
const { fontFamily: dm } = loadDM();

const C = {
  bg: "#0c0d10",
  ink: "#f4f1ea",
  muted: "rgba(244,241,234,0.62)",
  line: "rgba(244,241,234,0.12)",
  nasiko: "#e8a317", // warm amber — Nasiko signal
  drona: "#2dd4bf", // teal — DronaHQ signal
  danger: "#ff5c5c",
  save: "#3dd68c",
  card: "#16181e",
};

const springCfg = { damping: 18, stiffness: 90, mass: 0.7 };

const FadeUp: React.FC<{
  children: React.ReactNode;
  delay?: number;
  style?: React.CSSProperties;
}> = ({ children, delay = 0, style }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = spring({ frame: frame - delay, fps, config: springCfg });
  const y = interpolate(t, [0, 1], [28, 0]);
  const o = interpolate(t, [0, 1], [0, 1]);
  return (
    <div style={{ opacity: o, transform: `translateY(${y}px)`, ...style }}>
      {children}
    </div>
  );
};

const BrandPill: React.FC<{ label: string; color: string; delay?: number }> = ({
  label,
  color,
  delay = 0,
}) => (
  <FadeUp delay={delay}>
    <div
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 10,
        padding: "10px 18px",
        borderRadius: 999,
        border: `1px solid ${color}55`,
        background: `${color}14`,
        color,
        fontFamily: dm,
        fontSize: 22,
        fontWeight: 600,
        letterSpacing: "0.04em",
      }}
    >
      <span
        style={{
          width: 10,
          height: 10,
          borderRadius: "50%",
          background: color,
          boxShadow: `0 0 16px ${color}`,
        }}
      />
      {label}
    </div>
  </FadeUp>
);

const Backdrop: React.FC = () => {
  const frame = useCurrentFrame();
  const drift = interpolate(frame, [0, 2700], [0, 80], {
    extrapolateRight: "clamp",
  });
  return (
    <AbsoluteFill
      style={{
        background: `
          radial-gradient(1200px 700px at ${20 + drift * 0.1}% -10%, ${C.nasiko}22, transparent 55%),
          radial-gradient(900px 600px at ${85 - drift * 0.08}% 110%, ${C.drona}18, transparent 50%),
          ${C.bg}
        `,
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0,
          opacity: 0.18,
          backgroundImage:
            "linear-gradient(rgba(244,241,234,0.06) 1px, transparent 1px), linear-gradient(90deg, rgba(244,241,234,0.06) 1px, transparent 1px)",
          backgroundSize: "64px 64px",
          maskImage:
            "radial-gradient(ellipse at center, black 30%, transparent 75%)",
        }}
      />
    </AbsoluteFill>
  );
};

const ShotFrame: React.FC<{
  src: string;
  caption: string;
  accent: string;
  delay?: number;
}> = ({ src, caption, accent, delay = 0 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const t = spring({ frame: frame - delay, fps, config: springCfg });
  const scale = interpolate(t, [0, 1], [0.92, 1]);
  const o = interpolate(t, [0, 1], [0, 1]);
  const ken = interpolate(frame, [0, 120], [1, 1.04], {
    extrapolateRight: "clamp",
    easing: Easing.out(Easing.cubic),
  });
  return (
    <div
      style={{
        opacity: o,
        transform: `scale(${scale})`,
        width: "100%",
        maxWidth: 1500,
      }}
    >
      <div
        style={{
          borderRadius: 20,
          overflow: "hidden",
          border: `1px solid ${accent}44`,
          boxShadow: `0 40px 100px rgba(0,0,0,.55), 0 0 0 1px ${C.line}`,
          background: C.card,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "12px 16px",
            borderBottom: `1px solid ${C.line}`,
            background: "#12141a",
          }}
        >
          <span style={dot("#ff5f57")} />
          <span style={dot("#febc2e")} />
          <span style={dot("#28c840")} />
          <span
            style={{
              marginLeft: 14,
              fontFamily: dm,
              fontSize: 16,
              color: C.muted,
            }}
          >
            {caption}
          </span>
        </div>
        <div style={{ overflow: "hidden", height: 720 }}>
          <Img
            src={staticFile(src)}
            style={{
              width: "100%",
              height: "100%",
              objectFit: "cover",
              objectPosition: "top center",
              transform: `scale(${ken})`,
            }}
          />
        </div>
      </div>
    </div>
  );
};

const dot = (c: string): React.CSSProperties => ({
  width: 12,
  height: 12,
  borderRadius: "50%",
  background: c,
});

/* ── Scenes ─────────────────────────────────────────────────────────── */

const ColdOpen: React.FC = () => {
  const frame = useCurrentFrame();
  const pulse = interpolate(Math.sin(frame / 8), [-1, 1], [0.85, 1]);
  return (
    <AbsoluteFill
      style={{
        justifyContent: "center",
        alignItems: "center",
        fontFamily: syne,
      }}
    >
      <FadeUp>
        <div
          style={{
            fontSize: 28,
            letterSpacing: "0.28em",
            textTransform: "uppercase",
            color: C.muted,
            fontFamily: dm,
            marginBottom: 24,
          }}
        >
          Build-A-Thon · Nasiko × DronaHQ
        </div>
      </FadeUp>
      <FadeUp delay={6}>
        <div
          style={{
            fontSize: 96,
            fontWeight: 800,
            letterSpacing: "-0.04em",
            color: C.ink,
            textAlign: "center",
            lineHeight: 1.05,
          }}
        >
          Smart LLM Router
        </div>
      </FadeUp>
      <FadeUp delay={14}>
        <div
          style={{
            marginTop: 28,
            fontSize: 32,
            color: C.muted,
            fontFamily: dm,
            maxWidth: 900,
            textAlign: "center",
            lineHeight: 1.35,
            opacity: pulse,
          }}
        >
          Stop burning GPT‑4o on every prompt.
        </div>
      </FadeUp>
    </AbsoluteFill>
  );
};

const Problem: React.FC = () => (
  <AbsoluteFill style={{ padding: 100, justifyContent: "center" }}>
    <FadeUp>
      <div
        style={{
          fontFamily: dm,
          fontSize: 22,
          color: C.danger,
          letterSpacing: "0.2em",
          textTransform: "uppercase",
          marginBottom: 20,
        }}
      >
        The problem
      </div>
    </FadeUp>
    <FadeUp delay={4}>
      <div
        style={{
          fontFamily: syne,
          fontSize: 72,
          fontWeight: 800,
          letterSpacing: "-0.03em",
          color: C.ink,
          maxWidth: 1400,
          lineHeight: 1.1,
        }}
      >
        Greetings, summaries, and proofs
        <br />
        all hit the{" "}
        <span style={{ color: C.danger }}>same expensive model</span>.
      </div>
    </FadeUp>
    <div
      style={{
        display: "flex",
        gap: 24,
        marginTop: 56,
      }}
    >
      {["hi → GPT‑4o", "translate → GPT‑4o", "prove theorem → GPT‑4o"].map(
        (t, i) => (
          <FadeUp key={t} delay={12 + i * 6}>
            <div
              style={{
                padding: "22px 28px",
                borderRadius: 16,
                background: C.card,
                border: `1px solid ${C.line}`,
                fontFamily: dm,
                fontSize: 26,
                color: C.ink,
              }}
            >
              {t}
            </div>
          </FadeUp>
        ),
      )}
    </div>
  </AbsoluteFill>
);

const Partners: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const bridge = spring({ frame: frame - 20, fps, config: springCfg });
  const bridgeW = interpolate(bridge, [0, 1], [0, 220]);

  return (
    <AbsoluteFill
      style={{
        justifyContent: "center",
        alignItems: "center",
        padding: 80,
      }}
    >
      <FadeUp>
        <div
          style={{
            fontFamily: dm,
            fontSize: 22,
            letterSpacing: "0.22em",
            textTransform: "uppercase",
            color: C.muted,
            marginBottom: 40,
          }}
        >
          Who does what
        </div>
      </FadeUp>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 0,
        }}
      >
        <PartnerCard
          delay={4}
          title="Nasiko"
          role="Control plane"
          color={C.nasiko}
          points={[
            "A2A agent deploy & chat",
            "POST /v1/route cascade",
            "ACL · TokenOps · traces",
          ]}
        />
        <div
          style={{
            width: bridgeW,
            height: 3,
            background: `linear-gradient(90deg, ${C.nasiko}, ${C.drona})`,
            margin: "0 8px",
            borderRadius: 99,
            position: "relative",
            top: -20,
          }}
        />
        <PartnerCard
          delay={14}
          title="DronaHQ"
          role="Agents builder"
          color={C.drona}
          points={[
            "Same classify → tier → REST graph",
            "Frozen /v1/route contract",
            "Low-code side of the demo",
          ]}
        />
      </div>
      <FadeUp delay={28} style={{ marginTop: 48 }}>
        <div
          style={{
            fontFamily: dm,
            fontSize: 28,
            color: C.ink,
            textAlign: "center",
            maxWidth: 1100,
            lineHeight: 1.4,
          }}
        >
          One HTTP contract. Nasiko owns the router. DronaHQ owns the builder loop.
        </div>
      </FadeUp>
    </AbsoluteFill>
  );
};

const PartnerCard: React.FC<{
  title: string;
  role: string;
  color: string;
  points: string[];
  delay: number;
}> = ({ title, role, color, points, delay }) => (
  <FadeUp delay={delay}>
    <div
      style={{
        width: 520,
        padding: 40,
        borderRadius: 24,
        background: C.card,
        border: `1px solid ${color}55`,
        boxShadow: `0 0 60px ${color}18`,
      }}
    >
      <div
        style={{
          fontFamily: syne,
          fontSize: 48,
          fontWeight: 800,
          color,
          letterSpacing: "-0.03em",
        }}
      >
        {title}
      </div>
      <div
        style={{
          fontFamily: dm,
          fontSize: 20,
          color: C.muted,
          marginTop: 6,
          marginBottom: 28,
        }}
      >
        {role}
      </div>
      {points.map((p) => (
        <div
          key={p}
          style={{
            fontFamily: dm,
            fontSize: 22,
            color: C.ink,
            padding: "10px 0",
            borderTop: `1px solid ${C.line}`,
          }}
        >
          {p}
        </div>
      ))}
    </div>
  </FadeUp>
);

const Architecture: React.FC = () => {
  const steps = [
    { label: "Classify", sub: "heuristic / cheap LLM", color: C.ink },
    { label: "Tier", sub: "cheap · balanced · premium", color: C.drona },
    { label: "Nasiko /v1/route", sub: "provider cascade", color: C.nasiko },
    { label: "Savings", sub: "vs always GPT‑4o", color: C.save },
  ];
  return (
    <AbsoluteFill style={{ padding: 90, justifyContent: "center" }}>
      <FadeUp>
        <div
          style={{
            fontFamily: dm,
            fontSize: 22,
            letterSpacing: "0.2em",
            textTransform: "uppercase",
            color: C.muted,
            marginBottom: 18,
          }}
        >
          Architecture · product usage
        </div>
      </FadeUp>
      <FadeUp delay={4}>
        <div
          style={{
            fontFamily: syne,
            fontSize: 64,
            fontWeight: 800,
            color: C.ink,
            letterSpacing: "-0.03em",
            marginBottom: 48,
          }}
        >
          Console stays chat. Routing gets smart.
        </div>
      </FadeUp>
      <div style={{ display: "flex", gap: 18, alignItems: "stretch" }}>
        {steps.map((s, i) => (
          <React.Fragment key={s.label}>
            <FadeUp delay={10 + i * 7} style={{ flex: 1 }}>
              <div
                style={{
                  height: "100%",
                  padding: 28,
                  borderRadius: 18,
                  background: C.card,
                  border: `1px solid ${s.color}44`,
                }}
              >
                <div
                  style={{
                    fontFamily: dm,
                    fontSize: 16,
                    color: C.muted,
                    marginBottom: 10,
                  }}
                >
                  0{i + 1}
                </div>
                <div
                  style={{
                    fontFamily: syne,
                    fontSize: 32,
                    fontWeight: 700,
                    color: s.color,
                    marginBottom: 10,
                  }}
                >
                  {s.label}
                </div>
                <div style={{ fontFamily: dm, fontSize: 20, color: C.muted }}>
                  {s.sub}
                </div>
              </div>
            </FadeUp>
            {i < steps.length - 1 && (
              <FadeUp
                delay={14 + i * 7}
                style={{
                  alignSelf: "center",
                  color: C.muted,
                  fontSize: 36,
                  fontFamily: dm,
                }}
              >
                →
              </FadeUp>
            )}
          </React.Fragment>
        ))}
      </div>
      <FadeUp delay={42} style={{ marginTop: 40 }}>
        <div style={{ display: "flex", gap: 16 }}>
          <BrandPill label="Nasiko control plane" color={C.nasiko} />
          <BrandPill label="DronaHQ-compatible contract" color={C.drona} delay={6} />
        </div>
      </FadeUp>
    </AbsoluteFill>
  );
};

const NasikoConsole: React.FC = () => (
  <AbsoluteFill
    style={{
      padding: 60,
      justifyContent: "center",
      alignItems: "center",
      gap: 28,
    }}
  >
    <FadeUp>
      <div style={{ display: "flex", gap: 14, marginBottom: 8 }}>
        <BrandPill label="Nasiko console · A2A" color={C.nasiko} />
      </div>
    </FadeUp>
    <FadeUp delay={4}>
      <div
        style={{
          fontFamily: syne,
          fontSize: 48,
          fontWeight: 800,
          color: C.ink,
          letterSpacing: "-0.03em",
          marginBottom: 20,
          textAlign: "center",
        }}
      >
        Chat stays normal. Latency drops to ~10ms.
      </div>
    </FadeUp>
    <div style={{ display: "flex", gap: 28, width: "100%", justifyContent: "center" }}>
      <div style={{ flex: 1, maxWidth: 720 }}>
        <ShotFrame
          src="shots/console-before-slow.png"
          caption="Before · 20.0s hangs"
          accent={C.danger}
          delay={8}
        />
      </div>
      <div style={{ flex: 1, maxWidth: 720 }}>
        <ShotFrame
          src="shots/console-after-fast.png"
          caption="After · Nasiko-routed agent"
          accent={C.save}
          delay={16}
        />
      </div>
    </div>
  </AbsoluteFill>
);

const Analytics: React.FC = () => (
  <AbsoluteFill
    style={{
      padding: 50,
      justifyContent: "center",
      alignItems: "center",
    }}
  >
    <FadeUp>
      <div style={{ display: "flex", gap: 14, marginBottom: 16, justifyContent: "center" }}>
        <BrandPill label="Nasiko-powered analytics" color={C.nasiko} />
        <BrandPill label="Savings vs GPT‑4o" color={C.save} delay={4} />
      </div>
    </FadeUp>
    <FadeUp delay={4}>
      <div
        style={{
          fontFamily: syne,
          fontSize: 44,
          fontWeight: 800,
          color: C.ink,
          marginBottom: 18,
          letterSpacing: "-0.03em",
          textAlign: "center",
        }}
      >
        Prove the save — live tier mix & spend
      </div>
    </FadeUp>
    <ShotFrame
      src="shots/dashboard-live.png"
      caption="Smart Router · /ui dashboard"
      accent={C.nasiko}
      delay={8}
    />
  </AbsoluteFill>
);

const Contract: React.FC = () => {
  const nodes = [
    { t: "Classifier", c: C.ink },
    { t: "Tier JS", c: C.drona },
    { t: "REST /v1/route", c: C.nasiko },
    { t: "Savings", c: C.save },
  ];
  return (
    <AbsoluteFill style={{ padding: 90, justifyContent: "center" }}>
      <FadeUp>
        <div
          style={{
            fontFamily: dm,
            fontSize: 22,
            letterSpacing: "0.2em",
            textTransform: "uppercase",
            color: C.drona,
            marginBottom: 16,
          }}
        >
          DronaHQ Agents · shared graph
        </div>
      </FadeUp>
      <FadeUp delay={4}>
        <div
          style={{
            fontFamily: syne,
            fontSize: 58,
            fontWeight: 800,
            color: C.ink,
            letterSpacing: "-0.03em",
            maxWidth: 1400,
            lineHeight: 1.1,
            marginBottom: 40,
          }}
        >
          The same builder loop —{" "}
          <span style={{ color: C.drona }}>DronaHQ</span> points at{" "}
          <span style={{ color: C.nasiko }}>Nasiko /v1/route</span>.
        </div>
      </FadeUp>
      <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
        {nodes.map((n, i) => (
          <React.Fragment key={n.t}>
            <FadeUp delay={10 + i * 6}>
              <div
                style={{
                  padding: "26px 36px",
                  borderRadius: 16,
                  background: C.card,
                  border: `2px solid ${n.c}`,
                  fontFamily: syne,
                  fontSize: 28,
                  fontWeight: 700,
                  color: n.c,
                  minWidth: 200,
                  textAlign: "center",
                }}
              >
                {n.t}
              </div>
            </FadeUp>
            {i < nodes.length - 1 && (
              <FadeUp delay={14 + i * 6}>
                <div style={{ color: C.muted, fontSize: 32 }}>→</div>
              </FadeUp>
            )}
          </React.Fragment>
        ))}
      </div>
      <FadeUp delay={40} style={{ marginTop: 48 }}>
        <div
          style={{
            fontFamily: dm,
            fontSize: 26,
            color: C.muted,
            maxWidth: 1200,
            lineHeight: 1.45,
          }}
        >
          We own the router contract inside Nasiko. DronaHQ owns the low-code agent
          graph. One integration day — point ROUTER_BASE_URL at Nasiko.
        </div>
      </FadeUp>
    </AbsoluteFill>
  );
};

const Outro: React.FC = () => (
  <AbsoluteFill
    style={{
      justifyContent: "center",
      alignItems: "center",
      textAlign: "center",
      padding: 80,
    }}
  >
    <FadeUp>
      <div style={{ display: "flex", gap: 14, justifyContent: "center", marginBottom: 28 }}>
        <BrandPill label="@nasikolabs" color={C.nasiko} />
        <BrandPill label="@DronaHQ" color={C.drona} delay={4} />
      </div>
    </FadeUp>
    <FadeUp delay={6}>
      <div
        style={{
          fontFamily: syne,
          fontSize: 64,
          fontWeight: 800,
          color: C.ink,
          letterSpacing: "-0.03em",
          lineHeight: 1.1,
        }}
      >
        Smart routing on the Nasiko
        <br />
        control plane.
      </div>
    </FadeUp>
    <FadeUp delay={14}>
      <div
        style={{
          marginTop: 32,
          fontFamily: dm,
          fontSize: 28,
          color: C.muted,
        }}
      >
        github.com/Nasiko-Labs/nasiko/pull/182
      </div>
    </FadeUp>
  </AbsoluteFill>
);

export const Demo: React.FC = () => {
  return (
    <AbsoluteFill style={{ backgroundColor: C.bg }}>
      <Backdrop />
      <Sequence from={scene.coldOpen.from} durationInFrames={scene.coldOpen.dur}>
        <ColdOpen />
      </Sequence>
      <Sequence from={scene.problem.from} durationInFrames={scene.problem.dur}>
        <Problem />
      </Sequence>
      <Sequence from={scene.partners.from} durationInFrames={scene.partners.dur}>
        <Partners />
      </Sequence>
      <Sequence
        from={scene.architecture.from}
        durationInFrames={scene.architecture.dur}
      >
        <Architecture />
      </Sequence>
      <Sequence
        from={scene.nasikoConsole.from}
        durationInFrames={scene.nasikoConsole.dur}
      >
        <NasikoConsole />
      </Sequence>
      <Sequence from={scene.analytics.from} durationInFrames={scene.analytics.dur}>
        <Analytics />
      </Sequence>
      <Sequence from={scene.contract.from} durationInFrames={scene.contract.dur}>
        <Contract />
      </Sequence>
      <Sequence from={scene.outro.from} durationInFrames={scene.outro.dur}>
        <Outro />
      </Sequence>
    </AbsoluteFill>
  );
};
