#!/usr/bin/env python3
"""Train P2 request-classifier weights (hashed n-gram logistic regression)."""
import argparse, hashlib, json, math, random
from collections import defaultdict
from datetime import datetime, timezone
NUM_BUCKETS = 1 << 13
NUM_DENSE = 8
CLASSES = ["code_generation","code_understanding","technical_design","analytical_reasoning","writing","factual_lookup","general"]
N_COMPLEXITY = 5
OPENERS = {"write","implement","fix","debug","refactor","explain","rewrite","summarize","design","draft","analyze","calculate","describe","add","create","build","generate","review","convert","translate","list"}
SEED = 7
def fnv1a(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for b in data:
        h ^= b
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h
def hash_sign(gram: str) -> float:
    return 1.0 if fnv1a(("sign:" + gram).encode()) & 1 == 0 else -1.0
def word_tokens(text: str):
    out, cur = [], []
    for ch in text.lower():
        if ch.isalnum(): cur.append(ch)
        elif cur: out.append("".join(cur)); cur = []
    if cur: out.append("".join(cur))
    return out
def extract(query: str, context):
    combined = query + ("\n" + context if context else "")
    tokens = word_tokens(combined)
    grams = list(tokens) + [tokens[i]+" "+tokens[i+1] for i in range(len(tokens)-1)]
    low = list(combined.lower())
    for n in (3,4,5):
        if n <= len(low): grams += ["".join(low[i:i+n]) for i in range(len(low)-n+1)]
    norm = math.sqrt(len(grams)) if grams else 1.0
    by_bucket = defaultdict(float)
    for g in grams:
        by_bucket[fnv1a(g.encode()) % NUM_BUCKETS] += hash_sign(g) / norm
    hashed = sorted(by_bucket.items())
    tq = word_tokens(query)
    d = [0.0]*NUM_DENSE
    d[0]=math.log1p(len(tokens)); d[1]=math.log1p(len(list(query)))
    d[2]=1.0 if "?" in query else 0.0
    d[3]=1.0 if ("\u0060\u0060\u0060" in query or "\u0060" in query) else 0.0
    d[4]=1.0 if (tq and tq[0] in OPENERS) else 0.0
    d[5]=math.log1p(len(list(context)) if context else 0)
    d[6]=1.0 if any(c.isascii() and c.isdigit() for c in query) else 0.0
    d[7]=math.log1p(query.count("\n"))
    return hashed, d
def logits_of(h, d, bias, dense, hashed):
    out = []
    for k in range(len(bias)):
        acc = bias[k]
        dw = dense[k]
        for j in range(NUM_DENSE): acc += d[j]*dw[j]
        hw = hashed[k]
        for b, v in h:
            w = hw.get(b)
            if w is not None: acc += v*w
        out.append(acc)
    return out
def softmax(ls):
    m = max(ls); es = [math.exp(l-m) for l in ls]; s = sum(es)
    return [e/s for e in es]
def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--train", required=True)
    ap.add_argument("--out", required=True); ap.add_argument("--epochs", type=int, default=400)
    ap.add_argument("--lr", type=float, default=0.5); ap.add_argument("--l2", type=float, default=1e-4)
    a = ap.parse_args()
    rows = [json.loads(l) for l in open(a.train) if l.strip()]
    by_cls = defaultdict(list)
    for r in rows: by_cls[r["request_type"]].append(r)
    tr_rows, va_rows = [], []
    for c in CLASSES:
        for i, r in enumerate(sorted(by_cls[c], key=lambda r: r["id"])):
            (tr_rows if i % 5 < 4 else va_rows).append(r)
    rng = random.Random(SEED); rng.shuffle(tr_rows)
    tr = [(r,)+extract(r["query"], r.get("context")) for r in tr_rows]
    va = [(r,)+extract(r["query"], r.get("context")) for r in va_rows]
    K = len(CLASSES)
    tb=[0.0]*K; td=[[0.0]*NUM_DENSE for _ in range(K)]; th=[defaultdict(float) for _ in range(K)]
    cb=[0.0]*N_COMPLEXITY; cd=[[0.0]*NUM_DENSE for _ in range(N_COMPLEXITY)]; ch=[defaultdict(float) for _ in range(N_COMPLEXITY)]
    n=len(tr)
    for ep in range(a.epochs):
        gtb=[0.0]*K; gtd=[[0.0]*NUM_DENSE for _ in range(K)]; gth=[defaultdict(float) for _ in range(K)]
        gcb=[0.0]*N_COMPLEXITY; gcd=[[0.0]*NUM_DENSE for _ in range(N_COMPLEXITY)]; gch=[defaultdict(float) for _ in range(N_COMPLEXITY)]
        for r,h,d in tr:
            ty=CLASSES.index(r["request_type"]); pt=softmax(logits_of(h,d,tb,td,th))
            for k in range(K):
                e=pt[k]-(1.0 if k==ty else 0.0); gtb[k]+=e
                for j in range(NUM_DENSE): gtd[k][j]+=e*d[j]
                for b,v in h: gth[k][b]+=e*v
            cy=r["complexity"]-1; pc=softmax(logits_of(h,d,cb,cd,ch))
            for k in range(N_COMPLEXITY):
                e=pc[k]-(1.0 if k==cy else 0.0); gcb[k]+=e
                for j in range(NUM_DENSE): gcd[k][j]+=e*d[j]
                for b,v in h: gch[k][b]+=e*v
        for k in range(K):
            tb[k]-=a.lr*gtb[k]/n
            for j in range(NUM_DENSE): td[k][j]-=a.lr*(gtd[k][j]/n+a.l2*td[k][j])
            for b,g in gth[k].items(): th[k][b]-=a.lr*(g/n+a.l2*th[k].get(b,0.0))
        for k in range(N_COMPLEXITY):
            cb[k]-=a.lr*gcb[k]/n
            for j in range(NUM_DENSE): cd[k][j]-=a.lr*(gcd[k][j]/n+a.l2*cd[k][j])
            for b,g in gch[k].items(): ch[k][b]-=a.lr*(g/n+a.l2*ch[k].get(b,0.0))
    def predict(rs):
        yt=[]; yp=[]; ce=[]
        for r,h,d in rs:
            pt=softmax(logits_of(h,d,tb,td,th)); k=max(range(K),key=lambda i: pt[i])
            yt.append(CLASSES.index(r["request_type"])); yp.append(k)
            pc=softmax(logits_of(h,d,cb,cd,ch)); exp=sum((i+1)*p for i,p in enumerate(pc))
            ce.append(abs(round(exp)-r["complexity"]))
        return sum(x==y for x,y in zip(yt,yp))/len(yt), sum(ce)/len(ce)
    tr_acc,tr_mae=predict(tr); va_acc,va_mae=predict(va)
    best_t,best_nll=1.0,float("inf"); t=0.2
    while t<=5.0:
        nll=0.0
        for r,h,d in tr:
            ls=logits_of(h,d,tb,td,th); m=max(x/t for x in ls)
            es=[math.exp(x/t-m) for x in ls]; s=sum(es)
            nll-=math.log(es[CLASSES.index(r["request_type"])]/s)
        nll/=len(tr)
        if nll<best_nll: best_nll,best_t=nll,t
        t+=0.05
    T=round(best_t,3)
    dh=hashlib.sha256(open(a.train,"rb").read()).hexdigest()
    wire={"version":1,"provenance":{"trained_at_utc":datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),"train_count":len(tr),"val_count":len(va),"feature":"word1-2+char3-5 fnv1a/8192 signed l2norm +8dense (request_model.rs)","seed":SEED,"notes":"train_acc=%.3f val_acc=%.3f cxmae_tr=%.3f cxmae_va=%.3f T-nll=%.3f split=stratified-idmod5 sha=%s"%(tr_acc,va_acc,tr_mae,va_mae,best_nll,dh[:16])},"num_buckets":NUM_BUCKETS,"num_dense":NUM_DENSE,"request_types":CLASSES,"temperature":T,"type_bias":tb,"type_dense":td,"type_hashed":[{str(b):w for b,w in sorted(m.items()) if abs(w)>1e-12} for m in th],"complexity_bias":cb,"complexity_dense":cd,"complexity_hashed":[{str(b):w for b,w in sorted(m.items()) if abs(w)>1e-12} for m in ch]}
    json.dump(wire, open(a.out,"w"))
    print("train_acc=%.3f cxmae=%.3f | val_acc=%.3f cxmae=%.3f | T=%.3f nll=%.3f"%(tr_acc,tr_mae,va_acc,va_mae,T,best_nll))
    print("train=%d val=%d wrote %s"%(len(tr),len(va),a.out))
if __name__=="__main__": main()
