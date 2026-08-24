(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (has_root . %has_root.3) (is_full . %is_full.4)
    (notes . %notes.1) (record . %record.2) (reset . %reset.0))
  (contract-types)
  (kernel-declaration (%kernel.9 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%notes.1 (0) (exported #t) (MerkleTree 4 (tbytes 32))))
    (constructor () (tuple)))
  (circuit %record.2 (exported #t) (pure #f) (proof #t)
    ((%item.5 (tbytes 32))
      (%index.6 (tunsigned 18446744073709551615)))
    (ttuple)
    (seq (public-ledger %notes.1 update (0) insert (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (dup (n 2))
             (idx (cached #f) (pushPath #f) (path ((align 1 1))))
             (push
               (storage #t)
               (value (state-value cell (leaf-hash (var-ref %item.5)))))
             (ins (cached #f) (n 1)) (ins (cached #t) (n 1))
             (idx (cached #f) (pushPath #t) (path ((align 1 1))))
             (addi (immediate 1)) (ins (cached #t) (n 2)))
           (var-ref %item.5))
         (public-ledger %notes.1 update (0) insertIndex (ttuple)
           (instructions (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (idx (cached #f) (pushPath #t) (path ((align 0 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %index.6))))
             (push
               (storage #t)
               (value (state-value cell (leaf-hash (var-ref %item.5)))))
             (ins (cached #f) (n 2))
             (idx (cached #f) (pushPath #t) (path ((align 1 1))))
             (push
               (storage #f)
               (value (state-value cell (var-ref %index.6))))
             (addi (immediate 1)) (dup (n 1)) (dup (n 1)) (lt)
             (branch (skip 2)) (pop) (jmp (skip 2)) (swap (n 0)) (pop)
             (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
           (var-ref %item.5) (var-ref %index.6))
         (return (tuple))))
  (circuit %has_root.3 (exported #t) (pure #f) (proof #t)
    ((%digest.7 (tfield (field-native)))) (tboolean)
    (return
      (let* (((%tmp.8
                (tstruct MerkleTreeDigest (field (tfield (field-native))))) (new (tstruct
                                                                                   MerkleTreeDigest
                                                                                   (field
                                                                                     (tfield
                                                                                       (field-native))))
                                                                                 (var-ref
                                                                                   %digest.7))))
        (public-ledger %notes.1 read (0) checkRoot (tboolean)
          (instructions (dup (n 0))
            (idx (cached #f) (pushPath #f) (path ((align 0 1))))
            (idx (cached #f) (pushPath #f) (path ((align 0 1)))) (root)
            (push
              (storage #f)
              (value (state-value cell (var-ref %tmp.8))))
            (eq) (popeq (cached #t) (result (void))))
          (var-ref %tmp.8)))))
  (circuit %is_full.4 (exported #t) (pure #f) (proof #t) () (tboolean)
    (return
      (public-ledger %notes.1 read (0) isFull (tboolean)
        (instructions (dup (n 0))
          (idx (cached #f) (pushPath #f) (path ((align 0 1))))
          (idx (cached #f) (pushPath #f) (path ((align 1 1))))
          (push (storage #f) (value (state-value cell (align 16 8))))
          (lt) (neg) (popeq (cached #t) (result (void)))))))
  (circuit %reset.0 (exported #t) (pure #f) (proof #t) () (ttuple)
    (seq (public-ledger %notes.1 remove (0) resetToDefault (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push
               (storage #t)
               (value
                 (state-value
                   array
                   (state-value merkle-tree 4)
                   (state-value cell (align 0 8)))))
             (ins (cached #f) (n 1))))
         (return (tuple)))))
