"""Train-only grouped-CV temperature calibration for the frozen Strands type head.

Preparation and fitting never read H4/public/WildChat labels or predictions.
Rust exports the exact serving rubric and bounded state for selected training rows.
"""
import argparse,hashlib,json,math,urllib.request
from collections import Counter
from pathlib import Path
import numpy as np
from sklearn.model_selection import StratifiedGroupKFold
ROOT=Path(__file__).resolve().parents[2]
OUT=Path(__file__).resolve().parent/'calibration'
TYPES=['code_generation','code_understanding','technical_design','analytical_reasoning','writing','factual_lookup','general']
STRANDS_MIN_TYPE_PROBABILITY=(1+6*.4)/7
p=argparse.ArgumentParser(description=__doc__);p.add_argument('mode',choices=['prepare','collect','fit']);p.add_argument('--url',default='http://127.0.0.1:18099');a=p.parse_args()
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def grams(s):
 s=' '.join(s.casefold().split());return {s[i:i+5] for i in range(max(1,len(s)-4))}
def key(r):return hashlib.sha256(('20261003:'+r['id']).encode()).hexdigest()
def nll(prob,y,t):
 z=np.log(np.maximum(prob,1e-12))/t;z-=z.max(axis=1,keepdims=True)
 logp=z-np.log(np.exp(z).sum(axis=1,keepdims=True));return float(-logp[np.arange(len(y)),y].mean())
def scale(prob,t):
 z=np.log(np.maximum(prob,1e-12))/t;z-=z.max(axis=1,keepdims=True);p=np.exp(z);return p/p.sum(axis=1,keepdims=True)
def metric(prob,y):
 top=prob.max(axis=1);ok=prob.argmax(axis=1)==y;bins=np.minimum((top*10).astype(int),9)
 ece=sum(float((bins==i).mean())*abs(float(top[bins==i].mean()-ok[bins==i].mean())) for i in range(10) if (bins==i).any())
 trusted=top>=STRANDS_MIN_TYPE_PROBABILITY
 return dict(n=len(y),accuracy=float(ok.mean()),nll=nll(prob,y,1),ece=ece,top_label_brier=float(((top-ok)**2).mean()),multiclass_brier=float(((prob-np.eye(7)[y])**2).sum(axis=1).mean()),low_confidence=float((~trusted).mean()),trusted_accuracy=float(ok[trusted].mean()) if trusted.any() else None,confident_wrong=int((trusted&~ok).sum()))
OUT.mkdir(exist_ok=True)
if a.mode=='prepare':
 files=sorted((ROOT/'eval').glob('train-*.jsonl'));rows=[]
 for f in files:
  rows.extend(dict(json.loads(l),training_file=f.name) for l in f.read_text().splitlines() if l.strip())
 # One request per declared family, balanced by type; seeded digest ordering.
 selected=[];groups=set()
 for t in TYPES:
  chosen=[]
  for r in sorted((r for r in rows if r['request_type']==t),key=key):
   family=r.get('family',r['id'])
   if family in groups:continue
   groups.add(family);chosen.append(dict(r,family=family))
   if len(chosen)==40:break
  assert len(chosen)==40,t
  selected.extend(chosen)
 # Union any undeclared near-duplicate families across selected rows for CV.
 parent=list(range(len(selected)));shingles=[grams(r['query']) for r in selected]
 def find(i):
  while parent[i]!=i:parent[i]=parent[parent[i]];i=parent[i]
  return i
 for i in range(len(selected)):
  for j in range(i):
   union=shingles[i]|shingles[j]
   if len(shingles[i]&shingles[j])/max(1,len(union))>=.7:parent[find(i)]=find(j)
 for i,r in enumerate(selected):r['calibration_group']='cg-'+str(find(i))
 (OUT/'selected-training.jsonl').write_text(''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in selected))
 (OUT/'selection-manifest.json').write_text(json.dumps(dict(training_files={f.name:digest(f) for f in files},selected_sha256=digest(OUT/'selected-training.jsonl'),n=len(selected),groups=len({r['calibration_group'] for r in selected}),types=dict(Counter(r['request_type'] for r in selected)),source_files=dict(Counter(r['training_file'] for r in selected)),policy='40/type; at most one declared family; SHA256(seed:id) ordering; char5Jaccard>=0.7 grouped across selected requests; no final test labels read'),indent=2)+'\n')
 print('prepared',len(selected),'train-only requests',flush=True)
elif a.mode=='collect':
 rows=[json.loads(l) for l in (OUT/'requests.jsonl').read_text().splitlines()]
 health=json.load(urllib.request.urlopen(a.url+'/health',timeout=15))
 assert health.get('quantization_bits')==4,health
 manifest=dict(health=health,requests_sha256=digest(OUT/'requests.jsonl'),selection_sha256=digest(OUT/'selected-training.jsonl'),url=a.url)
 # Longest real bounded state is the readiness/warmup profile, before any scored requests.
 longest=max(rows,key=lambda r:len(r['body']['state']))['body']
 (OUT/'warmup-body.json').write_text(json.dumps(longest,indent=2)+'\n')
 urllib.request.urlopen(urllib.request.Request(a.url+'/v1/systemone',json.dumps(longest).encode(),{'Content-Type':'application/json'}),timeout=60).read()
 with (OUT/'responses.jsonl').open('w') as out:
  for i,r in enumerate(rows,1):
   req=urllib.request.Request(a.url+'/v1/systemone',json.dumps(r['body'],sort_keys=True).encode(),{'Content-Type':'application/json'})
   response=json.load(urllib.request.urlopen(req,timeout=30));answer=response['answers']['request_type']
   assert set(answer['probabilities'])==set(TYPES)
   out.write(json.dumps(dict(id=r['id'],request_type=r['request_type'],group=r['calibration_group'],answer=answer))+'\n');out.flush()
   if i%40==0:print('collected',i,'/',len(rows),flush=True)
 (OUT/'inference-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
else:
 rows=[json.loads(l) for l in (OUT/'responses.jsonl').read_text().splitlines()]
 assert len(rows)==280 and len({r['id'] for r in rows})==280
 y=np.array([TYPES.index(r['request_type']) for r in rows]);groups=np.array([r['group'] for r in rows])
 prob=np.array([[r['answer']['probabilities'][t] for t in TYPES] for r in rows]);prob/=prob.sum(axis=1,keepdims=True)
 grid=np.arange(.2,3.001,.02);choose=lambda p,y:float(min(grid,key=lambda t:nll(p,y,t)))
 oof=np.zeros_like(prob);folds=[]
 for tr,te in StratifiedGroupKFold(5,shuffle=True,random_state=7).split(prob,y,groups):
  assert not set(groups[tr])&set(groups[te])
  t=choose(prob[tr],y[tr]);oof[te]=scale(prob[te],t);folds.append(dict(n=len(te),temperature=t))
 original=metric(prob,y);calibrated=metric(oof,y)
 # P2 penalizes confidently wrong decisions. Better average NLL alone cannot
 # justify admitting additional errors above the probability-equivalent minimum
 # of the unchanged native 0.4 concentration floor.
 # This gate is decided on training-only CV, before final holdout evaluation.
 improved_nll=calibrated['nll']<original['nll']
 no_extra_confident_errors=calibrated['confident_wrong']<=original['confident_wrong']
 use_calibrated=improved_nll and no_extra_confident_errors
 temperature=choose(prob,y) if use_calibrated else 1.0
 report=dict(raw_probability=original,grouped_cv_calibrated=calibrated,folds=folds,selected_temperature=temperature,accepted_calibration=use_calibrated,admission_min_type_probability=STRANDS_MIN_TYPE_PROBABILITY,acceptance_policy='held-out training-group NLL improves AND confident_wrong at preserved Strands probability-equivalent admission minimum does not increase',notes='Probabilities, not native normalized concentration. CV grades held-out training families only. Argmax unchanged. No final evaluation set read.')
 (OUT/'cv-report.json').write_text(json.dumps(report,indent=2)+'\n')
 artifact=dict(version=1,type_temperature=round(temperature,6),provenance=dict(trainer='eval/strands/calibrate.py',accepted_calibration=use_calibrated,acceptance_policy=report['acceptance_policy'],training_rows=len(rows),cv='5-fold StratifiedGroupKFold, declared and near-duplicate family groups',responses_sha256=digest(OUT/'responses.jsonl'),requests_sha256=digest(OUT/'requests.jsonl'),training_files=json.loads((OUT/'selection-manifest.json').read_text())['training_files'],checkpoint=json.loads((OUT/'inference-manifest.json').read_text())['health'],raw_cv_nll=round(original['nll'],6),calibrated_cv_nll=round(calibrated['nll'],6)))
 (ROOT/'assets'/'strands_calibration.json').write_text(json.dumps(artifact,indent=2)+'\n')
 print(json.dumps(report,indent=2))
