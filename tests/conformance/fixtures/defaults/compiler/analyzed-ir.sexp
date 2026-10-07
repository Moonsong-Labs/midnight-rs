(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports (balances . %balances.18) (bump . %bump.19)
   (digest . %digest.16) (digests . %digests.17)
   (entry . %entry.14) (flag . %flag.15)
   (history . %history.12) (hits . %hits.13)
   (label . %label.10) (members . %members.11) (note . %note.8)
   (notes . %notes.9) (pair . %pair.6) (peer . %peer.7)
   (phase . %phase.4) (point . %point.5) (queue . %queue.2)
   (score . %score.3) (small . %small.0) (tag . %tag.1))
  (contract-types)
  (kernel-declaration (%kernel.21 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (public-ledger-array
        (%phase.4
          (0 0)
          (exported #t)
          (__compact_Cell (tenum Phase idle active closed)))
        (%entry.14
          (0 1)
          (exported #t)
          (__compact_Cell
            (tstruct
              Entry
              (count (tunsigned 65535))
              (flag (tboolean))
              (tag (tbytes 4)))))
        (%note.8
          (0 2)
          (exported #t)
          (__compact_Cell
            (tstruct
              Maybe
              (is_some (tboolean))
              (value (topaque "string")))))
        (%label.10
          (0 3)
          (exported #t)
          (__compact_Cell (topaque "string"))))
      (public-ledger-array
        (%digest.16
          (1 0)
          (exported #t)
          (__compact_Cell (tfield (field-native))))
        (%point.5
          (1 1)
          (exported #t)
          (__compact_Cell (tpoint (curve-jubjub))))
        (%flag.15 (1 2) (exported #t) (__compact_Cell (tboolean)))
        (%tag.1 (1 3) (exported #t) (__compact_Cell (tbytes 4)))
        (%small.0
          (1 4)
          (exported #t)
          (__compact_Cell (tunsigned 65535)))
        (%pair.6
          (1 5)
          (exported #t)
          (__compact_Cell (ttuple (tboolean) (tunsigned 65535))))
        (%digests.17
          (1 6)
          (exported #t)
          (__compact_Cell (tvector 2 (tfield (field-native)))))
        (%score.3
          (1 7)
          (exported #t)
          (__compact_Cell (talias #t Score (tunsigned 65535))))
        (%peer.7
          (1 8)
          (exported #t)
          (__compact_Cell
            (tcontract Peer (ping #f () (tfield (field-native))))))
        (%queue.2
          (1 9)
          (exported #t)
          (List (tunsigned 18446744073709551615)))
        (%members.11 (1 10) (exported #t) (Set (tbytes 32)))
        (%balances.18
          (1 11)
          (exported #t)
          (Map (tunsigned 255) (tunsigned 18446744073709551615)))
        (%hits.13 (1 12) (exported #t) (Counter))
        (%notes.9 (1 13) (exported #t) (MerkleTree 4 (tbytes 32)))
        (%history.12
          (1 14)
          (exported #t)
          (HistoricMerkleTree 4 (tbytes 32)))))
    (constructor () (tuple)))
  (export-typedef Phase () (tenum Phase idle active closed))
  (export-typedef
    Entry
    ()
    (tstruct
      Entry
      (count (tunsigned 65535))
      (flag (tboolean))
      (tag (tbytes 4))))
  (circuit %bump.19 (exported #t) (pure #f) (proof #t) () (ttuple)
    (seq (let* (((%tmp.20 (tunsigned 65535)) (safe-cast
                                               (tunsigned 65535)
                                               (tunsigned 1)
                                               '1)))
           (public-ledger %hits.13 update (1 12) increment (ttuple)
             (instructions
               (idx (cached #f)
                    (pushPath #t)
                    (path ((align 1 1) (align 12 1))))
               (addi (immediate (value->int (var-ref %tmp.20))))
               (ins (cached #t) (n 2)))
             (var-ref %tmp.20)))
         (return (tuple)))))
