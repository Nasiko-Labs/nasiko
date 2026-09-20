"""Small parser for observed Prometheus counters; missing is never zero."""
import math
import re


def counter(text: str, name: str):
    values=[]
    pattern=re.compile(r'^'+re.escape(name)+r'(?:\{[^\n]*\})?\s+([^\s]+)(?:\s+[^\s]+)?$')
    for line in text.splitlines():
        match=pattern.match(line)
        if match:
            value=float(match[1])
            if math.isfinite(value): values.append(value)
    return sum(values) if values else None


def delta(before: str, after: str, name: str):
    a,b=counter(before,name),counter(after,name)
    return b-a if a is not None and b is not None and b>=a else None


def usage_totals(records):
    prompt=cached=generated=0
    coverage=0
    for record in records:
        for event in record.get('events',[]):
            usage=event.get('usage') or {}
            prompt+=usage.get('prompt_tokens',0) or 0
            generated+=usage.get('completion_tokens',0) or 0
            detail=usage.get('prompt_tokens_details') or {}
            if 'cached_tokens' in detail:
                coverage+=1
                cached+=detail['cached_tokens'] or 0
    return {'prompt_tokens':prompt,'cached_tokens':cached,'generated_tokens':generated,
            'cached_fraction':cached/prompt if prompt and coverage else None,
            'requests_with_cache_usage':coverage}
