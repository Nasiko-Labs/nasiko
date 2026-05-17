'use client';

import { useEffect } from 'react';
import { motion, useMotionValue, useSpring, useTransform } from 'framer-motion';

const ICON_BG = 'linear-gradient(126.97deg, #0048ff 28.26%, #21D4FD 91.2%)';
const ICON_SHADOW = '0 10px 24px rgba(0, 72, 255, 0.45)';

function AnimatedNumber({
  value,
  suffix = '',
  prefix = '',
}: {
  value: number;
  suffix?: string;
  prefix?: string;
}) {
  const mv = useMotionValue(0);
  const spring = useSpring(mv, { stiffness: 80, damping: 20 });
  const display = useTransform(
    spring,
    (v) => `${prefix}${Math.round(v).toLocaleString()}${suffix}`,
  );
  useEffect(() => {
    mv.set(value);
  }, [mv, value]);
  return <motion.span>{display}</motion.span>;
}

export function StatCard({
  label,
  value,
  suffix,
  prefix,
  sub,
  delay = 0,
  icon,
}: {
  label: string;
  value: number;
  suffix?: string;
  prefix?: string;
  sub?: string;
  delay?: number;
  icon: React.ReactNode;
}) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay, duration: 0.5, ease: [0.22, 1, 0.36, 1] }}
      className="card-premium p-6"
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1 pt-0.5">
          <p
            className="mb-2 text-sm font-medium capitalize"
            style={{ color: 'rgba(255,255,255,0.55)' }}
          >
            {label}
          </p>
          <p className="flex flex-wrap items-baseline gap-2 leading-none">
            <span className="text-2xl font-bold tracking-tight text-white md:text-[26px]">
              <AnimatedNumber value={value} suffix={suffix ?? ''} prefix={prefix ?? ''} />
            </span>
            {sub ? (
              <span className="text-sm font-bold" style={{ color: '#01B574' }}>
                {sub}
              </span>
            ) : null}
          </p>
        </div>
        <div
          className="flex h-14 w-14 shrink-0 items-center justify-center rounded-[14px]"
          style={{ background: ICON_BG, boxShadow: ICON_SHADOW }}
        >
          {icon}
        </div>
      </div>
    </motion.div>
  );
}
