"""Fetch a deterministic, text-only P2 sample from public WildChat viewer rows."""
import concurrent.futures, hashlib, json, random, urllib.parse, urllib.request
from pathlib import Path
ROOT = Path(__file__).resolve().parent
OFFSETS = [10000, 100000, 200000, 300000, 400000]
BASE = 'https://datasets-server.huggingface.co/rows'
def fetch(offset):
    url = BASE + '?' + urllib.parse.urlencode(dict(dataset='allenai/WildChat-1M', config='default', split='train', offset=offset, length=100))
    with urllib.request.urlopen(url, timeout=90) as response:
        payload = response.read()
    return offset, url, hashlib.sha256(payload).hexdigest(), json.loads(payload)['rows']
with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
    pages = list(pool.map(fetch, OFFSETS))
seen = set(); examples = []; counts = {}; source = []
for offset, url, digest, rows in pages:
    candidates = []
    for item in rows:
        row = item['row']
        if row['language'] != 'English' or row.get('toxic') or row.get('redacted'):
            continue
        conv = row['conversation']
        eligible = [i for i,m in enumerate(conv) if m['role']=='user' and 5 <= len(m['content'].strip()) <= 1000]
        if not eligible:
            continue
        # One turn per conversation. Half the selected sample prioritizes a later
        # eligible user turn when available; otherwise use its initial user turn.
        idx = eligible[-1] if int(row['conversation_hash'][:8],16)%2 and len(eligible)>1 else eligible[0]
        q = conv[idx]['content']
        normalized = ' '.join(q.lower().split())
        if normalized in seen: continue
        context = '\n'.join(f"[{m['role']}] {m['content'][:250]}" for m in reversed(conv[:idx]) if m['role'] in ('user','assistant'))
        # Match serving: three most recent earlier turns, 1,500 total characters
        # after the latest query/header (eval_state applies its own bound).
        context = '\n'.join(f"[{m['role']}] {m['content'][:250]}" for m in list(reversed(conv[:idx]))[:3])
        candidates.append(dict(source_row=item['row_idx'], conversation_hash=row['conversation_hash'], user_turn=sum(m['role']=='user' for m in conv[:idx])+1, query=q, context=context, language='English'))
    random.Random(20261003 + offset).shuffle(candidates)
    selected=[]
    for case in candidates:
        key=' '.join(case['query'].lower().split())
        if key in seen: continue
        selected.append(case); seen.add(key)
        if len(selected)==20: break
    counts[str(offset)] = len(selected)
    examples.extend(selected)
    source.append(dict(offset=offset,url=url,response_sha256=digest))
for i,e in enumerate(examples,1): e['id']=f'wc-{i:03}'
assert len(examples)==100, counts
(ROOT/'sample-unlabelled.json').write_text(json.dumps(dict(schema_version=1,source='allenai/WildChat-1M',examples=examples),indent=2,ensure_ascii=False)+'\n')
revision=json.load(urllib.request.urlopen('https://huggingface.co/api/datasets/allenai/WildChat-1M',timeout=60))['sha']
(ROOT/'source-manifest.json').write_text(json.dumps(dict(dataset='allenai/WildChat-1M',dataset_revision=revision,license='ODC-BY',dataset_url='https://huggingface.co/datasets/allenai/WildChat-1M',sample_seed=20261003,pages=source,counts=counts,selection='20 English non-toxic non-redacted conversations per 100-row block; unique normalized query; selected user text 5..1000 chars; one turn per conversation; hash parity selects later turn when available',followups=sum(e['user_turn']>1 for e in examples)),indent=2)+'\n')
print('saved',len(examples),'requests;',sum(e['user_turn']>1 for e in examples),'follow-ups')
