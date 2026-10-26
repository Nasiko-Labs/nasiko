"""Observed task/run comparisons; list-price estimates are not invoices."""
import argparse
import json
from pathlib import Path
import random
import statistics
from agentkv.measurements import delta, usage_totals
from agentkv.dashboard import percentile

# Public list prices verified 2026-09-20; configured resource envelope.
GPU_RATE=.000694 + 8*.0000131 + 64*.00000222
VM_RATE=4*.00003942 + 8*.00000667
JEV_INPUT_RATE=.042/1_000_000


def interval(values, seed=17):
    if len(values)<3: return None
    rng=random.Random(seed)
    draws=sorted(statistics.mean(rng.choices(values,k=len(values))) for _ in range(2000))
    return [draws[49],draws[1949]]


def analyze(report):
    trials=report['trials']; records=report['records']
    for trial in trials:
        selected=[r for r in records if r['method']==trial['method'] and r['repetition']==trial['repetition']
                  and r.get('concurrency',trial['concurrency'])==trial['concurrency']]
        successes=sum(r['success'] for r in selected)
        tokens=sum((r.get('jev') or {}).get('usage',{}).get('input_tokens',0) or 0
                   for r in selected if isinstance((r.get('jev') or {}).get('usage',{}),dict))
        unknown_jev=any((r.get('jev') or {}).get('usage')=='unknown' for r in selected)
        unknown_coder=any(e.get('agent')=='coder' and not (e.get('usage') or {}).get('prompt_tokens')
                          for r in selected for e in r.get('events',[]))
        known_cost=trial['seconds']*(GPU_RATE+VM_RATE)+tokens*JEV_INPUT_RATE
        trial.update(measured_window_resource_cost_estimate=known_cost,
            cost_basis='steady_state_resource_envelope_excludes_startup_storage_network',
            cost_per_thousand=known_cost/successes*1000 if successes and not unknown_jev and not unknown_coder else None,
            unknown_jev_cost=unknown_jev,unknown_coder_usage=unknown_coder,usage=usage_totals(selected))
        before,after=trial.get('metrics_before',''),trial.get('metrics_after','')
        trial['engine_counter_deltas']={name:delta(before,after,'vllm:'+name) for name in (
            'prefix_cache_queries_total','prefix_cache_hits_total','num_preemptions_total',
            'request_prefill_time_seconds_sum','request_decode_time_seconds_sum','request_queue_time_seconds_sum')}
        predictions=[r for r in selected if r.get('repair_probability') is not None and
                     any(e['agent']=='tester' and e['status']=='completed' for e in r.get('events',[]))]
        trial['retrospective_repair_brier_score']=statistics.mean((r['repair_probability']-bool(r.get('repair_attempted')))**2
            for r in predictions) if predictions else None
        timely=[r for r in predictions if r.get('decision_after_test') is False]
        trial['timely_prediction_count']=len(timely)
        trial['repair_brier_score']=statistics.mean((r['repair_probability']-bool(r.get('repair_attempted')))**2
            for r in timely) if timely else None
        trial['acknowledged_workflows']=sum(any(a['status']=='prioritized' for a in r.get('engine_acknowledgements',[])) for r in selected)
    comparisons=[]
    for concurrency in sorted({t['concurrency'] for t in trials}):
        for baseline,method in [('A','B'),('B','C'),('C','E'),('D','E'),('A','E')]:
            base={(r['task_id'],r['repetition']):r for r in records if r['method']==baseline and r.get('concurrency',concurrency)==concurrency}
            target={(r['task_id'],r['repetition']):r for r in records if r['method']==method and r.get('concurrency',concurrency)==concurrency}
            pairs=sorted(base.keys()&target.keys())
            if not pairs: continue
            # A batch shares queue/cache state: independent repeats, not tokens/tasks, are the timing units.
            run_deltas={}
            for key in pairs:
                run_deltas.setdefault(key[1],[]).append(target[key]['seconds']-base[key]['seconds'])
            diffs=[statistics.mean(v) for v in run_deltas.values()]
            comparisons.append({'baseline':baseline,'method':method,'concurrency':concurrency,
                'pairs':len(pairs),'paired_trials':len(diffs),'mean_latency_delta_seconds':statistics.mean(diffs),
                'paired_trial_deltas_seconds':diffs,'trial_bootstrap_95_interval':interval(diffs),
                'baseline_passed':sum(base[k]['success'] for k in pairs),
                'method_passed':sum(target[k]['success'] for k in pairs)})
    report['comparisons']=comparisons
    report['method_summary']=[]
    for concurrency in sorted({t['concurrency'] for t in trials}):
        for method in sorted({t['method'] for t in trials}):
            runs=[t for t in trials if t['method']==method and t['concurrency']==concurrency]
            selected=[r for r in records if r['method']==method and r.get('concurrency',concurrency)==concurrency]
            if not runs or not selected: continue
            successes=sum(r['success'] for r in selected)
            elapsed=sum(t['seconds'] for t in runs)
            jev=[r['jev'] for r in selected if r.get('jev')]
            report['method_summary'].append({'method':method,'concurrency':concurrency,
                'repetitions':len(runs),'attempted':len(selected),'passed':successes,
                'repairs':sum(bool(r.get('repair_attempted')) for r in selected),
                'mean_seconds':statistics.mean(r['seconds'] for r in selected),
                'p50_seconds':percentile([r['seconds'] for r in selected],.5),
                'p95_seconds':percentile([r['seconds'] for r in selected],.95),
                'service_p50_seconds':percentile([r['service_seconds'] for r in selected],.5),
                'successes_per_hour':successes*3600/elapsed,
                'cost_per_thousand':sum(t['measured_window_resource_cost_estimate'] for t in runs)*1000/successes
                    if successes and not any(t['unknown_jev_cost'] or t.get('unknown_coder_usage') for t in runs) else None,
                'usage':usage_totals(selected),
                'acknowledged_workflows':sum(t['acknowledged_workflows'] for t in runs),
                'jev_requests':len(jev),'jev_unknown_usage':sum(j.get('usage')=='unknown' for j in jev),
                'jev_mean_seconds':statistics.mean(j['seconds'] for j in jev if 'seconds' in j) if any('seconds' in j for j in jev) else None,
                'timely_prediction_count':sum(t['timely_prediction_count'] for t in runs),
                'warmed_workflows':sum((r.get('warm') or {}).get('status')=='warmed' for r in selected),
                'skipped_warm_workflows':sum((r.get('warm') or {}).get('status')=='skipped' for r in selected),
                'retrospective_repair_brier_score':statistics.mean(t['retrospective_repair_brier_score'] for t in runs if t['retrospective_repair_brier_score'] is not None)
                    if any(t['retrospective_repair_brier_score'] is not None for t in runs) else None,
                'repair_brier_score':statistics.mean(t['repair_brier_score'] for t in runs if t['repair_brier_score'] is not None)
                    if any(t['repair_brier_score'] is not None for t in runs) else None})
    report['probe_summary']=[]
    for method in ('A','B'):
        selected=[p for p in report.get('prefill_probe',[]) if p['method']==method]
        if not selected: continue
        usage=usage_totals([{'events':[{'usage':p.get('usage')}]} for p in selected])
        report['probe_summary'].append({'method':method,'requests':len(selected),
            'mean_seconds':statistics.mean(p['seconds'] for p in selected),
            'p50_seconds':percentile([p['seconds'] for p in selected],.5),
            'p95_seconds':percentile([p['seconds'] for p in selected],.95),'usage':usage,
            'all_one_token':all((p.get('usage') or {}).get('completion_tokens')==1 for p in selected)})
    import hashlib
    report['analysis_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    gpu_start=report.get('gpu_started_at'); end=report.get('completed_at')
    report['resource_ledger']={
        'rates_usd_per_second':{'gpu_function_configured_resources':GPU_RATE,'nasiko_vm_configured_resources':VM_RATE},
        'cpu_vm_wall_seconds':report.get('cpu_sandbox_wall_seconds'),
        'gpu_process_window_seconds':end-gpu_start if end and gpu_start else None,
        'known_process_window_estimate_usd':(end-gpu_start)*GPU_RATE+report.get('cpu_sandbox_wall_seconds',0)*VM_RATE if end and gpu_start else None,
        'exclusions':['allocation before process startup','post-report teardown','earlier failed development attempts',
                      'dashboard','volume storage and network','unreported Jev usage'],
        'reconciled_invoice':False}
    report['notes']=[
        'Authored 20-task utility-repair pilot, not SWE-bench. Repository tests are visible to the model.',
        'Latency includes client queueing for a fixed task batch. Concurrency cohorts are separate.',
        'Costs estimate configured resources during each measured trial; shared startup/build, storage and network are excluded. No total-bill saving is claimed.',
        'A/B use stock eviction under the same instrumentation. C/D/E add eviction preferences, never hard pins.',
        'D uses four disjoint calibration tasks. E probabilities are uncalibrated. Jev runs concurrently with inference.',
        'E asks at most two Noul questions in one request: repair within two transitions, and later LLM reuse of the shared prefix. Warming prefills an exact later prompt only while the GPU is idle and is recorded separately from task success.',
        'Twenty tasks cannot establish a two-percentage-point quality margin. Tail latency and bootstrap intervals are provisional.',
    ]
    return report


def main():
    parser=argparse.ArgumentParser();parser.add_argument('path');args=parser.parse_args()
    path=Path(args.path);report=analyze(json.loads(path.read_text()));path.write_text(json.dumps(report,indent=2))
    for trial in report['trials']:
        print(json.dumps({k:trial[k] for k in ['method','repetition','concurrency','seconds','cost_per_thousand','usage','acknowledged_workflows']}))
    for comparison in report['comparisons']: print(json.dumps(comparison))

if __name__=='__main__':main()
