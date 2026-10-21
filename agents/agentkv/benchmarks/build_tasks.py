"""Construct the original, frozen utility-repair pilot (not SWE-bench)."""
import hashlib
import json
from pathlib import Path

SPECS = [
("clamp", "numbers", "Clamp value to the inclusive low/high interval; raise ValueError if low exceeds high.", "def clamp(value, low, high):\n    return min(low, max(high, value))", "assert clamp(5,0,10)==5\nassert clamp(-1,0,10)==0\nassert clamp(11,0,10)==10"),
("median", "numbers", "Return the median of a nonempty numeric list without changing it; raise ValueError on empty input.", "def median(values):\n    return sorted(values)[len(values)//2]", "assert median([3,1,2])==2\nassert median([1,4])==2.5\na=[3,1]; median(a); assert a==[3,1]"),
("ceil_div", "numbers", "Compute mathematical ceiling of integer a divided by positive integer b; reject b <= 0.", "def ceil_div(a,b):\n    return a//b", "assert ceil_div(5,2)==3\nassert ceil_div(-5,2)==-2\nassert ceil_div(0,3)==0"),
("moving_average", "numbers", "Return averages of complete sliding windows of width k; reject nonpositive k, return [] when k exceeds length.", "def moving_average(values,k):\n    return [sum(values[i:i+k])/k for i in range(len(values))]", "assert moving_average([1,2,3,4],2)==[1.5,2.5,3.5]\nassert moving_average([1],2)==[]"),
("normalize", "numbers", "Divide each nonnegative weight by their sum; return all zeros if the sum is zero.", "def normalize(weights):\n    return [w/max(weights) for w in weights]", "assert normalize([1,3])==[0.25,0.75]\nassert normalize([0,0])==[0,0]\nassert normalize([])==[]"),
("slugify", "text", "Lowercase text, replace each run of non-ASCII-alphanumeric characters with one hyphen, trim hyphens. No imports.", "def slugify(text):\n    return text.lower().replace(' ','-')", "assert slugify(' Hello,  WORLD! ')== 'hello-world'\nassert slugify('a___b')=='a-b'\nassert slugify('')==''"),
("common_prefix", "text", "Return longest common prefix of strings; return empty string for empty input.", "def common_prefix(strings):\n    return strings[0]", "assert common_prefix(['flower','flow','flight'])=='fl'\nassert common_prefix([])==''\nassert common_prefix(['abc',''])==''"),
("word_counts", "text", "Count whitespace-separated words case-insensitively. Punctuation remains part of a word.", "def word_counts(text):\n    return {word:1 for word in text.split()}", "assert word_counts('Hi hi THERE')=={'hi':2,'there':1}\nassert word_counts('')=={}"),
("truncate", "text", "Limit string to max_length including a trailing '...' if truncated. For lengths 0,1,2 return that many dots when truncated.", "def truncate(text,max_length):\n    return text[:max_length]+'...'", "assert truncate('abcdef',5)=='ab...'\nassert truncate('abc',5)=='abc'\nassert truncate('abc',1)=='.'\nassert truncate('abc',0)==''"),
("is_palindrome", "text", "Check palindrome ignoring case and characters for which isalnum() is false.", "def is_palindrome(text):\n    return text==text[::-1]", "assert is_palindrome('A man, a plan, a canal: Panama!')\nassert not is_palindrome('ab')\nassert is_palindrome('')"),
("unique", "collections", "Deduplicate hashable values preserving their first-occurrence order.", "def unique(values):\n    return list(set(values))", "assert unique([3,1,3,2,1])==[3,1,2]\nassert unique([])==[]"),
("chunks", "collections", "Split a list into consecutive chunks of at most n elements; raise ValueError for n <= 0.", "def chunks(values,n):\n    return [values[i:i+n] for i in range(len(values))]", "assert chunks([1,2,3,4,5],2)==[[1,2],[3,4],[5]]\nassert chunks([],2)==[]"),
("flatten", "collections", "Flatten one level of lists, preserving order and retaining nested inner lists.", "def flatten(groups):\n    return groups", "assert flatten([[1,2],[],[3]])==[1,2,3]\nassert flatten([[[1]]])==[[1]]"),
("group_by", "collections", "Group dictionaries by a specified key, skipping dictionaries lacking that key. Preserve item order.", "def group_by(items,key):\n    return {item[key]:item for item in items}", "assert group_by([{'a':1,'v':2},{'a':1,'v':3},{}],'a')=={1:[{'a':1,'v':2},{'a':1,'v':3}]}\nassert group_by([],'a')=={}"),
("transpose", "collections", "Transpose a rectangular list of rows. Return [] for []; reject ragged input with ValueError.", "def transpose(rows):\n    return rows", "assert transpose([[1,2],[3,4]])==[[1,3],[2,4]]\nassert transpose([])==[]\nassert transpose([[],[]])==[]"),
("merge_intervals", "ranges", "Merge overlapping or touching [start,end] intervals; sort output, do not mutate input.", "def merge_intervals(intervals):\n    return sorted(intervals)", "assert merge_intervals([[3,4],[1,3],[7,8]])==[[1,4],[7,8]]\nassert merge_intervals([])==[]"),
("overlap", "ranges", "Return overlap length of two half-open intervals [a,b) and [c,d), zero if disjoint.", "def overlap(a,b,c,d):\n    return min(b,d)-max(a,c)", "assert overlap(0,3,2,5)==1\nassert overlap(0,1,3,4)==0\nassert overlap(0,1,1,2)==0"),
("binary_search", "ranges", "Return index of first occurrence of target in a sorted list, or -1 when absent. Use logarithmic search.", "def binary_search(values,target):\n    return values.index(target)", "assert binary_search([1,2,2,3],2)==1\nassert binary_search([1,3],2)==-1\nassert binary_search([],2)==-1"),
("range_sum", "ranges", "Given inclusive integer endpoints start/end, sum that range; return zero if start exceeds end.", "def range_sum(start,end):\n    return sum(range(start,end))", "assert range_sum(1,3)==6\nassert range_sum(3,1)==0\nassert range_sum(-2,2)==0"),
("missing_ranges", "ranges", "List missing inclusive intervals between low and high from sorted distinct in-range integers.", "def missing_ranges(values,low,high):\n    return []", "assert missing_ranges([1,3,7],1,8)==[[2,2],[4,6],[8,8]]\nassert missing_ranges([],1,3)==[[1,3]]\nassert missing_ranges([1],1,1)==[]"),
]


# Boundary cases are part of each repository's test suite, not token padding.
EDGE_TESTS = {
    "clamp": "assert clamp(0,0,0)==0\nassert clamp(10,0,10)==10\ntry:\n    clamp(1,2,0)\n    assert False, 'reversed interval must fail'\nexcept ValueError:\n    pass",
    "median": "assert median([-4,-2])==-3\nassert median([7])==7\ntry:\n    median([])\n    assert False, 'empty input must fail'\nexcept ValueError:\n    pass",
    "ceil_div": "assert ceil_div(-4,2)==-2\nassert ceil_div(1,7)==1\nfor b in [0,-1]:\n    try:\n        ceil_div(3,b)\n        assert False, 'nonpositive divisor must fail'\n    except ValueError:\n        pass",
    "moving_average": "assert moving_average([],2)==[]\nassert moving_average([2,4],1)==[2,4]\nfor k in [0,-1]:\n    try:\n        moving_average([1],k)\n        assert False, 'nonpositive width must fail'\n    except ValueError:\n        pass",
    "normalize": "assert normalize([4])==[1]\nassert normalize([0,2,0])==[0,1,0]\nx=[2,2]; normalize(x); assert x==[2,2]\nassert abs(sum(normalize([1,2,3]))-1)<1e-12",
    "slugify": "assert slugify('---')==''\nassert slugify('A1--B2')=='a1-b2'\nassert slugify('café cats')=='caf-cats'\nassert slugify('a\\tb\\nc')=='a-b-c'\nassert slugify('already-good')=='already-good'",
    "common_prefix": "assert common_prefix(['same','same'])=='same'\nassert common_prefix(['one'])=='one'\nassert common_prefix(['abc','xyz'])==''\nassert common_prefix(['','abc'])==''\nassert common_prefix(['a','ab','abc'])=='a'",
    "word_counts": "assert word_counts('one\\tone\\ntwo')=={'one':2,'two':1}\nassert word_counts(' A a! A ')=={'a':2,'a!':1}\nassert word_counts('   ')=={}\nassert word_counts('42 42')=={'42':2}",
    "truncate": "assert truncate('abcdef',3)=='...'\nassert truncate('abc',2)=='..'\nassert truncate('',0)==''\nassert truncate('abc',3)=='abc'\nassert truncate('abcdef',6)=='abcdef'\nassert truncate('abcdef',4)=='a...'",
    "is_palindrome": "assert is_palindrome('!!!')\nassert is_palindrome('0P0')\nassert not is_palindrome('0P')\nassert is_palindrome('No lemon, no melon')\nassert is_palindrome('x')\nassert not is_palindrome('python')",
    "unique": "assert unique(['b','a','b'])==['b','a']\nassert unique([None,None,1])==[None,1]\nassert unique([(1,2),(1,2),(2,1)])==[(1,2),(2,1)]\nx=[2,1,2]; unique(x); assert x==[2,1,2]",
    "chunks": "assert chunks([1,2],5)==[[1,2]]\nassert chunks([1,2],1)==[[1],[2]]\nfor n in [0,-2]:\n    try:\n        chunks([],n)\n        assert False, 'nonpositive chunk size must fail'\n    except ValueError:\n        pass",
    "flatten": "assert flatten([])==[]\nassert flatten([[],[]])==[]\nassert flatten([[None],['a','b']])==[None,'a','b']\nassert flatten([[[1,2]],[[3]]])==[[1,2],[3]]\nx=[[1],[2]]; flatten(x); assert x==[[1],[2]]",
    "group_by": "assert group_by([{},{}],'a')=={}\nassert group_by([{'a':None}],'a')=={None:[{'a':None}]}\nx=[{'a':2},{'a':1},{'a':2}]; out=group_by(x,'a'); assert list(out)==[2,1]\nassert out[2]==[x[0],x[2]]",
    "transpose": "assert transpose([[1,2,3]])==[[1],[2],[3]]\nassert transpose([[1],[2]])==[[1,2]]\ntry:\n    transpose([[1,2],[3]])\n    assert False, 'ragged rows must fail'\nexcept ValueError:\n    pass",
    "merge_intervals": "assert merge_intervals([[1,10],[2,3]])==[[1,10]]\nassert merge_intervals([[1,2],[2,3],[3,4]])==[[1,4]]\nassert merge_intervals([[2,2]])==[[2,2]]\nx=[[3,4],[1,2]]; merge_intervals(x); assert x==[[3,4],[1,2]]",
    "overlap": "assert overlap(-5,0,-3,2)==3\nassert overlap(1,1,0,2)==0\nassert overlap(0,10,2,3)==1\nassert overlap(2,3,0,10)==1\nassert overlap(0,5,0,5)==5\nassert overlap(3,4,0,1)==0",
    "binary_search": "assert binary_search([2,2,2,2],2)==0\nassert binary_search([1],1)==0\nassert binary_search([1],0)==-1\nassert binary_search([1,2,3,4],4)==3\nassert binary_search([-3,-1,0],-1)==1\nassert binary_search([1,2,3],7)==-1",
    "range_sum": "assert range_sum(4,4)==4\nassert range_sum(-5,-3)==-12\nassert range_sum(0,0)==0\nassert range_sum(0,100)==5050\nassert range_sum(10,-10)==0\nassert range_sum(-1,1)==0",
    "missing_ranges": "assert missing_ranges([2],1,3)==[[1,1],[3,3]]\nassert missing_ranges([1,2,3],1,3)==[]\nassert missing_ranges([-2,0],-3,1)==[[-3,-3],[-1,-1],[1,1]]\nassert missing_ranges([],0,0)==[[0,0]]",
}
SPECS = [(n,r,g,s,t+'\n'+EDGE_TESTS[n]) for n,r,g,s,t in SPECS]


def main():
    tasks=[]
    for name, repo, goal, source, tests in SPECS:
        context = '\n\n'.join('# File: '+n+'.py\n# Contract: '+g+'\n'+s+'\n\n# File: test_'+n+'.py\n'+t for n,r,g,s,t in SPECS if r==repo)
        tasks.append({"id":name,"repository":repo,"goal":goal,"source":source,"tests":tests,
                      "context":context,"context_sha256":hashlib.sha256(context.encode()).hexdigest()})
    # Fixed round-robin repository order exposes interleaved handoffs without padding.
    tasks=[tasks[i+j] for i in range(5) for j in (0,5,10,15)]
    Path(__file__).with_name('tasks.json').write_text(json.dumps(tasks,indent=2)+'\n')

if __name__=='__main__': main()
