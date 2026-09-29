from types import SimpleNamespace
import unittest
from agentkv.retention import ResidentGroup, prioritize


class Queue:
    def __init__(self, blocks):
        self.order = list(blocks)
    def remove(self, block):
        self.order.remove(block)
    def append(self, block):
        self.order.append(block)


def pool():
    blocks = [SimpleNamespace(block_id=i, is_null=i == 0, ref_cnt=0,
                              prev_free_block=True, next_free_block=True) for i in range(21)]
    return SimpleNamespace(blocks=blocks, num_gpu_blocks=21,
        cached_block_hashes_by_block={i: {bytes([i])} for i in range(1,21)},
        free_block_queue=Queue(blocks[1:]))


class RetentionTests(unittest.TestCase):
    def test_complete_group_moves_without_changing_references_or_membership(self):
        p = pool()
        group = ResidentGroup("r", {1: frozenset([b'\x01']), 2: frozenset([b'\x02'])})
        before = set(b.block_id for b in p.free_block_queue.order)
        ack = prioritize(p, {"r": group}, [{"request_id":"r","score":1,"expires_at":20}], 10)
        self.assertEqual(ack[0]["status"], "prioritized")
        self.assertEqual(set(b.block_id for b in p.free_block_queue.order), before)
        self.assertEqual({b.block_id for b in p.free_block_queue.order[-2:]}, {1,2})
        self.assertTrue(all(b.ref_cnt == 0 for b in p.blocks))

    def test_one_active_or_reclaimed_member_rejects_whole_group(self):
        for change in ("active", "reclaimed"):
            p = pool()
            group = ResidentGroup("r", {1: frozenset([b'\x01']), 2: frozenset([b'\x02'])})
            before = list(p.free_block_queue.order)
            if change == "active": p.blocks[2].ref_cnt = 1
            else: p.cached_block_hashes_by_block[2] = {b'new-generation'}
            ack = prioritize(p, {"r":group}, [{"request_id":"r","score":1,"expires_at":20}],10)
            self.assertEqual(ack[0]["status"], "active_or_reclaimed")
            self.assertEqual(before, p.free_block_queue.order)

    def test_expiry_and_quota_do_not_partially_move_a_group(self):
        p=pool()
        group=ResidentGroup("r", {i:frozenset([bytes([i])]) for i in range(1,7)})
        hints=[{"request_id":"r","score":1,"expires_at":20}]
        self.assertEqual(prioritize(p,{"r":group},hints,10)[0]["status"],"quota")
        self.assertEqual(prioritize(p,{"r":group},hints,21),[])

    def test_native_primary_hashes_are_resident_without_partial_aliases(self):
        p=pool()
        p.cached_block_hashes_by_block={}
        p.blocks[1].block_hash=b'primary'
        group=ResidentGroup('r',{1:frozenset([b'primary'])})
        hints=[{'request_id':'r','score':1,'expires_at':20}]
        self.assertEqual(prioritize(p,{'r':group},hints,10)[0]['status'],'prioritized')
        p.blocks[1].block_hash=b'reused'
        self.assertEqual(prioritize(p,{'r':group},hints,10)[0]['status'],'active_or_reclaimed')

    def test_overlapping_groups_share_quota_and_do_not_duplicate_free_blocks(self):
        p=pool()
        groups={name:ResidentGroup(name,{i:frozenset([bytes([i])]) for i in ids})
                for name,ids in [('first',[1,2,3]),('second',[3,4,5])]}
        hints=[{'request_id':name,'score':score,'expires_at':20} for name,score in [('first',2),('second',1)]]
        acks=prioritize(p,groups,hints,10)
        self.assertTrue(all(a['status']=='prioritized' for a in acks))
        self.assertEqual(len({b.block_id for b in p.free_block_queue.order}),20)
        self.assertEqual(len(p.free_block_queue.order),20)
        self.assertEqual({b.block_id for b in p.free_block_queue.order[-5:]},{1,2,3,4,5})
