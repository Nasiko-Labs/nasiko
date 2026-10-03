"""Score frozen AI annotations; never writes or changes labels."""
import argparse,collections,hashlib,json,math
from pathlib import Path
ROOT=Path(__file__).resolve().parent
BACKENDS=('regex','laya','strands')
ALLOWED_BACKENDS=('regex','local','laya','strands','strands_raw','hybrid')
DISPLAY_NAMES={'regex':'Regex','local':'Local','laya':'Laya','strands':'Strands 4-bit','strands_raw':'Strands raw 4-bit','hybrid':'Hybrid'}

def parse_args(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    destination=parser.add_mutually_exclusive_group()
    destination.add_argument('--output-name',default='results')
    destination.add_argument('--out',type=Path)
    parser.add_argument('--backends',nargs='+',choices=ALLOWED_BACKENDS,default=list(BACKENDS))
    args=parser.parse_args(argv)
    if len(args.backends)!=len(set(args.backends)):
        parser.error('--backends must not contain duplicates')
    return args

def read_predictions(path,backend,repeat,manifest):
    expected=next((r.get('predictions_sha256') for r in manifest.get('runs',[])
                   if r['backend']==backend and r['repeat']==repeat),None)
    if expected is not None and hashlib.sha256(path.read_bytes()).hexdigest()!=expected:
        raise ValueError(f'prediction provenance mismatch: {path.name}')
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]

def deadline_title(manifest,backends):
    budgets={manifest.get('timeouts_ms',{}).get(backend) for backend in backends}
    budgets.discard(None)
    if len(budgets)==1:
        return f'{next(iter(budgets))} ms deadline'
    return 'per-backend deadlines' if budgets else 'local backends'

TYPES=['code_generation','code_understanding','technical_design','analytical_reasoning','writing','factual_lookup','general']
def fraction(xs): return sum(xs)/len(xs) if xs else None
def metrics(pred,gold):
    by_id={r['id']:r for r in gold}
    assert len(pred)==len(gold) and {r['id'] for r in pred}==set(by_id)
    pairs=[(by_id[r['id']],r) for r in pred]; n=len(pairs)
    correct=[g['request_type']==r['request_type'] for g,r in pairs]
    lat=sorted(r['latency_us'] for g,r in pairs)
    quantile=lambda p:lat[math.ceil(p*n)-1]/1000
    per_type={}; confusion={t:{u:0 for u in TYPES} for t in TYPES}
    for g,r in pairs: confusion[g['request_type']][r['request_type']]+=1
    for t in TYPES:
        tp=confusion[t][t]; count=sum(confusion[t].values()); predicted=sum(confusion[s][t] for s in TYPES)
        per_type[t]=dict(n=count,f1=2*tp/(count+predicted) if count+predicted else 0,recall=tp/count if count else None)
    bins=[[] for _ in range(10)]
    for (g,r),ok in zip(pairs,correct):
        assert r['request_type'] in TYPES and 1<=r['complexity']<=5 and 0<=r['confidence']<=1
        bins[min(int(r['confidence']*10),9)].append((r['confidence'],int(ok)))
    ece=sum(len(b)/n*abs(sum(c for c,o in b)/len(b)-sum(o for c,o in b)/len(b)) for b in bins if b)
    below_floor=lambda r:r['source']!='regex' and r['fallback_reason'] is None and r['confidence']<0.4
    # New binaries expose the actual router admission policy. A probability is
    # not interchangeable with the raw Strands concentration score at the same floor.
    low=lambda r:r['source']!='regex' and r['fallback_reason'] is None and r.get('low_confidence',below_floor(r))
    trusted=[(g,r) for g,r in pairs if not low(r) and r['fallback_reason'] is None]
    hard=[(g,r) for g,r in pairs if g['complexity']>=4]
    slices={}
    for name,condition in [('initial',lambda g:g['user_turn']==1),('followup',lambda g:g['user_turn']>1),('unambiguous',lambda g:not g['ambiguous']),('ambiguous',lambda g:g['ambiguous']),('deduplicated',lambda g:g['id'] not in {'wc-026','wc-060','wc-072','wc-079'})]:
        subset=[(g,r) for g,r in pairs if condition(g)]
        slices[name]=dict(n=len(subset),accuracy=fraction([g['request_type']==r['request_type'] for g,r in subset]))
    return dict(n=n,correct=sum(correct),accuracy=fraction(correct),macro_f1=sum(v['f1'] for v in per_type.values())/7,alternative_aware_accuracy=fraction([r['request_type'] in g['acceptable_types'] for g,r in pairs]),complexity_exact=fraction([g['complexity']==r['complexity'] for g,r in pairs]),complexity_within1=fraction([abs(g['complexity']-r['complexity'])<=1 for g,r in pairs]),complexity_mae=sum(abs(g['complexity']-r['complexity']) for g,r in pairs)/n,hard_n=len(hard),hard_recall=fraction([r['complexity']>=4 for g,r in hard]),ece=ece,top_label_brier=sum((r['confidence']-int(ok))**2 for (g,r),ok in zip(pairs,correct))/n,p50_ms=quantile(.5),p95_ms=quantile(.95),p99_ms=quantile(.99),fallbacks=sum(r['fallback_reason'] is not None for g,r in pairs),below_confidence_floor=sum(below_floor(r) for g,r in pairs),low_confidence=sum(low(r) for g,r in pairs),trusted_n=len(trusted),trusted_accuracy=fraction([g['request_type']==r['request_type'] for g,r in trusted]),confident_wrong_count=sum(g['request_type']!=r['request_type'] for g,r in trusted),slices=slices,per_type=per_type,confusion=confusion)
def main(argv=None):
    a=parse_args(argv)
    OUTPUT=(a.out if a.out is not None else ROOT/a.output_name).expanduser().resolve()
    path=ROOT/'sample-labelled.json'
    assert hashlib.sha256(path.read_bytes()).hexdigest()==(ROOT/'labels-frozen.sha256').read_text().split()[0]
    gold=json.loads(path.read_text())['examples']; by_id={r['id']:r for r in gold}
    assert len(by_id)==len(gold)
    manifest=json.loads((OUTPUT/'run-manifest.json').read_text())
    recorded=manifest.get('requested_backends',manifest.get('order'))
    if recorded is not None and any(backend not in recorded for backend in a.backends):
        raise ValueError('requested scoring backend is absent from the run manifest')
    results={}
    for backend in a.backends:
        runs=[]
        for repeat in [1,2]:
            p=OUTPUT/f'{backend}-run{repeat}.jsonl'
            runs.append(read_predictions(p,backend,repeat,manifest))
        nonlat=lambda r:{k:v for k,v in r.items() if k!='latency_us'}
        results[backend]=metrics(runs[0],gold)
        results[backend]['repeat2']=metrics(runs[1],gold)
        results[backend]['changed_classifications']=[a['id'] for a,b in zip(*runs) if nonlat(a)!=nonlat(b)]
        failures=[]
        for pred in runs[0]:
            g=by_id[pred['id']]
            if g['request_type']!=pred['request_type']:
                failures.append(dict(id=g['id'],gold_type=g['request_type'],predicted_type=pred['request_type'],acceptable_types=g['acceptable_types'],confidence=pred['confidence'],ambiguous=g['ambiguous'],query=g['query']))
        (OUTPUT/f'{backend}-errors.json').write_text(json.dumps(failures,indent=2,ensure_ascii=False)+'\n')
    (OUTPUT/'summary.json').write_text(json.dumps(results,indent=2)+'\n')
    rows=[('Type accuracy',lambda r:f"{100*r['accuracy']:.1f}% ({r['correct']}/100)"),('Macro-F1',lambda r:f"{r['macro_f1']:.3f}"),('Unambiguous type accuracy (63)',lambda r:f"{100*r['slices']['unambiguous']['accuracy']:.1f}%"),('Near-duplicate-filtered accuracy (96)',lambda r:f"{100*r['slices']['deduplicated']['accuracy']:.1f}%"),('Alternative-aware type accuracy',lambda r:f"{100*r['alternative_aware_accuracy']:.1f}%"),('Initial-turn accuracy (63)',lambda r:f"{100*r['slices']['initial']['accuracy']:.1f}%"),('Follow-up accuracy (37)',lambda r:f"{100*r['slices']['followup']['accuracy']:.1f}%"),('Complexity exact',lambda r:f"{100*r['complexity_exact']:.1f}%"),('Complexity within ±1',lambda r:f"{100*r['complexity_within1']:.1f}%"),('Complexity MAE',lambda r:f"{r['complexity_mae']:.2f}"),('Hard recall (10)',lambda r:f"{100*r['hard_recall']:.1f}%"),('Returned-confidence ECE (semantics differ)',lambda r:f"{r['ece']:.3f}"),('Top-label Brier',lambda r:f"{r['top_label_brier']:.3f}"),('Latency p50',lambda r:f"{r['p50_ms']:.3f} ms"),('Latency p95',lambda r:f"{r['p95_ms']:.3f} ms"),('Latency p99',lambda r:f"{r['p99_ms']:.3f} ms"),('Error/timeout fallbacks',lambda r:str(r['fallbacks'])),('Confidence <0.4 (regex exempt)',lambda r:str(r['below_confidence_floor'])),('Safe-default policy requests',lambda r:str(r['low_confidence'])),('Accuracy among policy-accepted types',lambda r:(f"{100*r['trusted_accuracy']:.1f}% ({r['trusted_n']} accepted)" if r['trusted_accuracy'] is not None else "N/A (0 accepted)")),('Accepted wrong predictions',lambda r:str(r['confident_wrong_count'])),('Changed classifications on repeat',lambda r:str(len(r['changed_classifications'])))]
    manifest=json.loads((OUTPUT/'run-manifest.json').read_text())
    lines=[f"# Real WildChat P2 evaluation — {deadline_title(manifest,a.backends)}",'', '**Exploratory evaluation against pre-inference Codex annotations; not independent human gold.**','', '| Metric | '+' | '.join(DISPLAY_NAMES[b] for b in a.backends)+' |','|---|'+'---:|'*len(a.backends)]
    for title,formatter in rows: lines.append('| '+title+' | '+' | '.join(formatter(results[b]) for b in a.backends)+' |')
    lines += ['', '## Repeat 2', '', '| Backend | Accuracy | p50 | p95 | Fallbacks | Low confidence |', '|---|---:|---:|---:|---:|---:|']
    for backend,result in results.items():
     r=result['repeat2']; lines.append(f"| {backend} | {100*r['accuracy']:.1f}% | {r['p50_ms']:.3f} ms | {r['p95_ms']:.3f} ms | {r['fallbacks']} | {r['low_confidence']} |")
    lines += ['', 'Safe-default policy uses the returned low_confidence flag when present. Optimized Strands exposes probability confidence and preserves native concentration admission with an additional selected-probability minimum of (1+6*0.4)/7; literal confidence <0.4 alone is not its full routing policy.', '', 'Accuracy and confidence metrics include all recorded outputs, including operational fallback classifications where present. Accepted-type metrics exclude failure fallbacks; a failed backend does not provide a native model prediction.', '', 'Classification changes compare the complete outputs excluding latency. A change in fallback state is an operational change, not necessarily model nondeterminism.', '']
    lines+=['','## Type F1 and support','', '| Type | n | '+' | '.join(DISPLAY_NAMES[b] for b in a.backends)+' |','|---|---:|'+'---:|'*len(a.backends)]
    for t in TYPES:
     lines.append('| '+t+' | '+str(results[a.backends[0]]['per_type'][t]['n'])+' | '+' | '.join(f"{results[b]['per_type'][t]['f1']:.3f}" for b in a.backends)+' |')
    lines+=['','## Run provenance','', 'Requested backend order: '+', '.join(a.backends)+'.', f"Binary SHA-256: `{manifest.get('binary_sha256','not recorded')}`.", f"Frozen-label SHA-256: `{manifest.get('labels_sha256','not recorded')}`.", 'Source snapshots, endpoint mappings, runtime and sidecar residency metadata, calibration decisions, and prediction hashes are recorded in [run-manifest.json](run-manifest.json).', '']
    (OUTPUT/'report.md').write_text('\n'.join(lines).rstrip()+'\n')
    print('\n'.join(lines))

if __name__=='__main__':
    main()
