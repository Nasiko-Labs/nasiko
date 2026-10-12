import type { ApprovalRequest } from "../../shared/types.js";

export function ApprovalModal({
  request,
  onDecide,
}: {
  request: ApprovalRequest;
  onDecide: (approved: boolean) => void;
}) {
  return (
    <div className="fixed inset-0 bg-black/70 flex items-center justify-center z-50">
      <div className="w-[460px] rounded-lg border border-medium/50 bg-panel shadow-2xl">
        <div className="px-5 py-4 border-b border-edge flex items-center gap-2">
          <span className="text-medium text-lg">⚠</span>
          <h3 className="font-bold text-white">Agent Requests Approval</h3>
          <span className="ml-auto text-[10px] text-accent">
            🛡 Nasiko · ASK
          </span>
        </div>
        <div className="p-5 space-y-3 text-sm">
          <Field label="Tool" value={request.tool} />
          {request.args.to && (
            <Field label="Destination" value={String(request.args.to)} />
          )}
          <div>
            <div className="text-[10px] uppercase text-slate-500 mb-1">
              Policy
            </div>
            <p className="text-slate-300 leading-relaxed">{request.reason}</p>
          </div>
          <div className="text-[11px] text-slate-500">
            This action was intercepted by Nasiko's <b>ask</b> stance. A human
            must decide.
          </div>
        </div>
        <div className="px-5 py-4 border-t border-edge flex gap-3">
          <button
            onClick={() => onDecide(false)}
            className="flex-1 py-2 rounded bg-critical/20 border border-critical/50 text-critical font-semibold hover:bg-critical/30"
          >
            ✕ Block
          </button>
          <button
            onClick={() => onDecide(true)}
            className="flex-1 py-2 rounded bg-low/20 border border-low/50 text-low font-semibold hover:bg-low/30"
          >
            ✓ Approve
          </button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between">
      <span className="text-[10px] uppercase text-slate-500">{label}</span>
      <span className="font-mono text-white">{value}</span>
    </div>
  );
}
