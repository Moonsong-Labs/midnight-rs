(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports
    (hash_entries . %hash_entries.0)
    (tag_cell . %tag_cell.1))
  (contract-types)
  (kernel-declaration (%kernel.5 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%tag_cell.1
        (0)
        (exported #t)
        (__compact_Cell (tbytes 32))))
    (constructor () (tuple)))
  (export-typedef
    Entry
    ()
    (tstruct Entry (a (tunsigned 4294967295)) (b (tbytes 32))))
  (native %persistentHash.2
    (entry "__compactRuntime.persistentHash" circuit)
    (type-arguments
      (tvector
        3
        (tstruct Entry (a (tunsigned 4294967295)) (b (tbytes 32)))))
    ((%value.6
       (tvector
         3
         (tstruct
           Entry
           (a (tunsigned 4294967295))
           (b (tbytes 32))))))
    (tbytes 32))
  (circuit %hash_entries.0 (exported #t) (pure #f) (proof #t)
    ((%entries.3
       (tvector
         3
         (tstruct
           Entry
           (a (tunsigned 4294967295))
           (b (tbytes 32))))))
    (tbytes 32)
    (let* (((%h.4 (tbytes 32)) (call
                                 %persistentHash.2
                                 (var-ref %entries.3))))
      (seq (public-ledger %tag_cell.1 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %h.4))))
               (ins (cached #f) (n 1)))
             (var-ref %h.4))
           (return (var-ref %h.4))))))
