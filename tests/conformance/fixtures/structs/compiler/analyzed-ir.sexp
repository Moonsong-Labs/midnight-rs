(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (commit_struct . %commit_struct.3) (hash_odd . %hash_odd.4)
    (hash_struct . %hash_struct.1) (scratch . %scratch.2)
    (tag_cell . %tag_cell.0))
  (contract-types)
  (kernel-declaration (%kernel.17 () (exported #f) (Kernel)))
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
  (native %transientCommit.13
    (entry "__compactRuntime.transientCommit" circuit)
    (type-arguments
      (tstruct
        Point
        (x (tunsigned 4294967295))
        (flag (tboolean))
        (label (tbytes 32))))
    ((%value.18
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32))))
      (%rand.19 (tfield (field-native))))
    (tfield (field-native)))
  (native %persistentHash.8
    (entry "__compactRuntime.persistentHash" circuit)
    (type-arguments
      (tstruct
        Point
        (x (tunsigned 4294967295))
        (flag (tboolean))
        (label (tbytes 32))))
    ((%value.20
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32)))))
    (tbytes 32))
  (native %persistentHash.5
    (entry "__compactRuntime.persistentHash" circuit)
    (type-arguments
      (tstruct
        Odd
        (small (tunsigned 16777215))
        (medium (tunsigned 281474976710655))
        (ranged (tunsigned 999999))))
    ((%value.21
       (tstruct
         Odd
         (small (tunsigned 16777215))
         (medium (tunsigned 281474976710655))
         (ranged (tunsigned 999999)))))
    (tbytes 32))
  (native %persistentCommit.10
    (entry "__compactRuntime.persistentCommit" circuit)
    (type-arguments
      (tstruct
        Point
        (x (tunsigned 4294967295))
        (flag (tboolean))
        (label (tbytes 32))))
    ((%value.22
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32))))
      (%rand.23 (tbytes 32)))
    (tbytes 32))
  (circuit %hash_struct.1 (exported #t) (pure #f) (proof #t)
    ((%p.9
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32)))))
    (tbytes 32)
    (let* (((%h.11 (tbytes 32)) (call
                                  %persistentHash.8
                                  (var-ref %p.9))))
      (let* (((%c.12 (tbytes 32)) (call
                                    %persistentCommit.10
                                    (var-ref %p.9)
                                    (var-ref %h.11))))
        (seq (public-ledger %tag_cell.0 write (0) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 0 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %c.12))))
                 (ins (cached #f) (n 1)))
               (var-ref %c.12))
             (return (var-ref %c.12))))))
  (circuit %commit_struct.3 (exported #t) (pure #f) (proof #t)
    ((%p.14
       (tstruct
         Point
         (x (tunsigned 4294967295))
         (flag (tboolean))
         (label (tbytes 32))))
      (%r.15 (tfield (field-native))))
    (tfield (field-native))
    (let* (((%c.16 (tfield (field-native))) (call
                                              %transientCommit.13
                                              (var-ref %p.14)
                                              (var-ref %r.15))))
      (seq (public-ledger %scratch.2 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %c.16))))
               (ins (cached #f) (n 1)))
             (var-ref %c.16))
           (return (var-ref %c.16)))))
  (circuit %hash_odd.4 (exported #t) (pure #f) (proof #t)
    ((%o.6
       (tstruct
         Odd
         (small (tunsigned 16777215))
         (medium (tunsigned 281474976710655))
         (ranged (tunsigned 999999)))))
    (tbytes 32)
    (let* (((%h.7 (tbytes 32)) (call
                                 %persistentHash.5
                                 (var-ref %o.6))))
      (seq (public-ledger %tag_cell.0 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %h.7))))
               (ins (cached #f) (n 1)))
             (var-ref %h.7))
           (return (var-ref %h.7))))))
