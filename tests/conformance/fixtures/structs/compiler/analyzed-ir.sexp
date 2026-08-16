(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (commit_struct . %commit_struct.3) (hash_odd . %hash_odd.4)
    (hash_struct . %hash_struct.1) (scratch . %scratch.2)
    (tag_cell . %tag_cell.0))
  (contract-types)
  (kernel-declaration (%kernel.23 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%tag_cell.0 (0) (exported #t) (__compact_Cell (tbytes 32)))
      (%scratch.2
        (1)
        (exported #t)
        (__compact_Cell (tfield (field-native)))))
    (constructor () (tuple)))
  (export-typedef
    Point
    ()
    (tstruct
      Point
      (x (tunsigned 4294967295))
      (flag (tboolean))
      (label (tbytes 32))))
  (export-typedef
    Odd
    ()
    (tstruct
      Odd
      (small (tunsigned 16777215))
      (medium (tunsigned 281474976710655))
      (ranged (tunsigned 999999))))
  (native
    %transientCommit.16
    (entry "__compactRuntime.transientCommit" circuit)
    ((%value.20
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32))))
      (%rand.21 (tfield (field-native))))
    (tfield (field-native)))
  (native
    %persistentHash.10
    (entry "__compactRuntime.persistentHash" circuit)
    ((%value.22
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32)))))
    (tbytes 32))
  (native
    %persistentHash.7
    (entry "__compactRuntime.persistentHash" circuit)
    ((%value.17
       (tstruct
         Odd
         (small (tunsigned 16777215))
         (medium (tunsigned 281474976710655))
         (ranged (tunsigned 999999)))))
    (tbytes 32))
  (native
    %persistentCommit.12
    (entry "__compactRuntime.persistentCommit" circuit)
    ((%value.18
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32))))
      (%rand.19 (tbytes 32)))
    (tbytes 32))
  (circuit %hash_struct.1 (exported #t) (pure #f) (proof #t)
    ((%p.8
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32)))))
    (tbytes 32)
    (let* (((%h.9 (tbytes 32)) (call
                                 %persistentHash.10
                                 (var-ref %p.8))))
      (let* (((%c.11 (tbytes 32)) (call
                                    %persistentCommit.12
                                    (var-ref %p.8)
                                    (var-ref %h.9))))
        (seq (public-ledger %tag_cell.0 (0) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 0 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %c.11))))
                 (ins (cached #f) (n 1)))
               (var-ref %c.11))
             (return (var-ref %c.11))))))
  (circuit %commit_struct.3 (exported #t) (pure #f) (proof #t)
    ((%p.13
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32))))
      (%r.14 (tfield (field-native))))
    (tfield (field-native))
    (let* (((%c.15 (tfield (field-native))) (call
                                              %transientCommit.16
                                              (var-ref %p.13)
                                              (var-ref %r.14))))
      (seq (public-ledger %scratch.2 (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %c.15))))
               (ins (cached #f) (n 1)))
             (var-ref %c.15))
           (return (var-ref %c.15)))))
  (circuit %hash_odd.4 (exported #t) (pure #f) (proof #t)
    ((%o.5
       (tstruct
         Odd
         (small (tunsigned 16777215))
         (medium (tunsigned 281474976710655))
         (ranged (tunsigned 999999)))))
    (tbytes 32)
    (let* (((%h.6 (tbytes 32)) (call
                                 %persistentHash.7
                                 (var-ref %o.5))))
      (seq (public-ledger %tag_cell.0 (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %h.6))))
               (ins (cached #f) (n 1)))
             (var-ref %h.6))
           (return (var-ref %h.6))))))
