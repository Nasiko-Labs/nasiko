"""Apply Codex's pre-inference P2 annotations; no classifier outputs are read."""
import hashlib,json
from pathlib import Path
ROOT=Path(__file__).resolve().parent
TYPES=dict(cg='code_generation',cu='code_understanding',td='technical_design',ar='analytical_reasoning',wr='writing',fl='factual_lookup',ge='general')
# primary type, complexity, optional defensible alternative type(s).
# These annotations were made by reading query/context before any predictions.
ANNOTATIONS='''
001 ar 3 td
002 wr 3
003 wr 4
004 ar 3 fl
005 td 4 cg
006 wr 2
007 cg 3
008 cu 3 ar
009 wr 2
010 cg 3
011 cg 4 td
012 wr 4
013 ge 1
014 wr 2
015 wr 2
016 cg 3
017 fl 1
018 fl 1
019 wr 3
020 wr 3
021 cg 2
022 ar 3
023 wr 4
024 cu 3 cg
025 cg 2
026 cg 2
027 ar 3 fl
028 wr 4
029 td 3 cu
030 wr 3
031 ar 3
032 fl 1
033 fl 2 wr
034 ar 2 fl
035 cu 3 cg
036 cu 3 cg
037 fl 1
038 wr 2
039 cg 3
040 ar 2 fl
041 fl 2 cu
042 cg 3
043 ar 3 td
044 wr 2
045 wr 4
046 wr 3 ge
047 wr 2 ar
048 ge 2 wr
049 ge 1
050 fl 1 cu
051 cg 2 fl
052 cg 2
053 cu 2
054 cg 3 cu
055 wr 2
056 wr 2
057 fl 1
058 wr 4
059 cu 3
060 wr 3 ge
061 cg 3
062 cg 2 cu
063 wr 3
064 wr 2
065 fl 2 ar
066 cg 2 fl
067 fl 2
068 wr 3
069 ge 4 wr
070 wr 2
071 wr 2
072 wr 2
073 fl 2 ge
074 ar 3
075 wr 3
076 wr 2 fl
077 wr 2
078 ge 2 wr
079 wr 3
080 wr 3
081 cu 3 fl
082 td 3 ar
083 wr 2
084 cg 3
085 wr 3
086 wr 2
087 fl 3 ar
088 wr 2
089 fl 2
090 cu 2 fl
091 wr 3
092 td 3 cu
093 td 4 cg
094 wr 1
095 wr 3
096 wr 3
097 wr 1
098 td 3 cu
099 cu 3
100 fl 2 ge
'''
rows=json.loads((ROOT/'sample-unlabelled.json').read_text())['examples']
by_id={r['id']:r for r in rows}
for line in ANNOTATIONS.strip().splitlines():
    num,kind,cx,*alternatives=line.split()
    row=by_id['wc-'+num]
    row.update(request_type=TYPES[kind],complexity=int(cx),acceptable_types=[TYPES[kind]]+[TYPES[k] for k in alternatives],ambiguous=bool(alternatives),annotation_source='Codex, before inference; not independent human gold')
assert len(rows)==len(ANNOTATIONS.strip().splitlines())==100
payload=dict(schema_version=1,purpose='Exploratory P2 evaluation on real public user requests; AI-annotated labels',source='allenai/WildChat-1M',examples=rows)
p=ROOT/'sample-labelled.json'; p.write_text(json.dumps(payload,indent=2,ensure_ascii=False)+'\n')
(ROOT/'labels-frozen.sha256').write_text(hashlib.sha256(p.read_bytes()).hexdigest()+'  sample-labelled.json\n')
from collections import Counter
print('types',dict(Counter(r['request_type'] for r in rows)))
print('complexity',dict(Counter(r['complexity'] for r in rows)))
print('ambiguous',sum(r['ambiguous'] for r in rows))
