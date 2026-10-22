(analyzed-ir (compiler-version "0.35.103-dev")
  (language-version "0.27.0") (runtime-version "0.20.101")
  (exports
    (flat_points . %flat_points.28)
    (hash_entries . %hash_entries.29)
    (points . %points.26)
    (tag_cell . %tag_cell.27))
  (contract-types)
  (kernel-declaration (%kernel.41 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%tag_cell.27
        (0)
        (exported #t)
        (__compact_Cell (tbytes 32)))
      (%points.26
        (1)
        (exported #t)
        (__compact_Cell (tvector 2 (tpoint (curve-jubjub))))))
    (constructor () (tuple)))
  (export-typedef
    Entry
    ()
    (tstruct Entry (a (tunsigned 4294967295)) (b (tbytes 32))))
  (native %persistentHash.30
    (entry "__compactRuntime.persistentHash" circuit)
    (type-arguments
      (tvector
        3
        (tstruct Entry (a (tunsigned 4294967295)) (b (tbytes 32)))))
    ((%value.42
       (tvector
         3
         (tstruct
           Entry
           (a (tunsigned 4294967295))
           (b (tbytes 32))))))
    (tbytes 32))
  (native %ecAdd.38 (entry "__compactRuntime.ecAdd" circuit)
    (type-arguments)
    ((%a.43 (tpoint (curve-jubjub)))
      (%b.44 (tpoint (curve-jubjub))))
    (tpoint (curve-jubjub)))
  (native %ecMulGenerator.33
    (entry "__compactRuntime.ecMulGenerator" circuit)
    (type-arguments)
    ((%b.45 (tfield (field-scalar (curve-jubjub)))))
    (tpoint (curve-jubjub)))
  (circuit %hash_entries.29 (exported #t) (pure #f) (proof #t)
    ((%entries.31
       (tvector
         3
         (tstruct
           Entry
           (a (tunsigned 4294967295))
           (b (tbytes 32))))))
    (tbytes 32)
    (let* (((%h.32 (tbytes 32)) (call
                                  %persistentHash.30
                                  (var-ref %entries.31))))
      (seq (public-ledger %tag_cell.27 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %h.32))))
               (ins (cached #f) (n 1)))
             (var-ref %h.32))
           (return (var-ref %h.32)))))
  (circuit %flat_points.28 (exported #t) (pure #f) (proof #t)
    ((%a.34 (tfield (field-native)))
      (%b.35 (tfield (field-native))))
    (ttuple
      (tboolean)
      (tboolean)
      (tvector 3 (tpoint (curve-jubjub))))
    (let* (((%p.36 (tpoint (curve-jubjub))) (call
                                              %ecMulGenerator.33
                                              (cast-to-field
                                                (field-scalar
                                                  (curve-jubjub))
                                                (tfield (field-native))
                                                (var-ref %a.34)))))
      (let* (((%q.37 (tpoint (curve-jubjub))) (call
                                                %ecMulGenerator.33
                                                (cast-to-field
                                                  (field-scalar
                                                    (curve-jubjub))
                                                  (tfield (field-native))
                                                  (var-ref %b.35)))))
        (let* (((%before.39 (tboolean)) (== (ttuple
                                              (tpoint (curve-jubjub))
                                              (tpoint (curve-jubjub)))
                                            (public-ledger %points.26 read (1) read
                                              (tvector
                                                2
                                                (tpoint (curve-jubjub)))
                                              (instructions
                                                (dup (n 0))
                                                (idx (cached #f)
                                                     (pushPath #f)
                                                     (path ((align 1 1))))
                                                (popeq
                                                  (cached #f)
                                                  (result (void)))))
                                            (tuple
                                              (single (var-ref %p.36))
                                              (single (var-ref %q.37))))))
          (seq (let* (((%tmp.40 (tvector 2 (tpoint (curve-jubjub)))) (tuple
                                                                       (single
                                                                         (var-ref
                                                                           %p.36))
                                                                       (single
                                                                         (var-ref
                                                                           %q.37)))))
                 (public-ledger %points.26 write (1) write (ttuple)
                   (instructions
                     (push
                       (storage #f)
                       (value (state-value cell (align 1 1))))
                     (push
                       (storage #t)
                       (value (state-value cell (var-ref %tmp.40))))
                     (ins (cached #f) (n 1)))
                   (var-ref %tmp.40)))
               (return
                 (tuple
                   (single (var-ref %before.39))
                   (single
                     (== (ttuple
                           (tpoint (curve-jubjub))
                           (tpoint (curve-jubjub)))
                         (public-ledger %points.26 read (1) read
                           (tvector 2 (tpoint (curve-jubjub)))
                           (instructions
                             (dup (n 0))
                             (idx (cached #f)
                                  (pushPath #f)
                                  (path ((align 1 1))))
                             (popeq (cached #f) (result (void)))))
                         (tuple
                           (single (var-ref %p.36))
                           (single (var-ref %q.37)))))
                   (single
                     (vector
                       (spread
                         2
                         (public-ledger %points.26 read (1) read
                           (tvector 2 (tpoint (curve-jubjub)))
                           (instructions
                             (dup (n 0))
                             (idx (cached #f)
                                  (pushPath #f)
                                  (path ((align 1 1))))
                             (popeq (cached #f) (result (void))))))
                       (single
                         (call
                           %ecAdd.38
                           (var-ref %p.36)
                           (var-ref %q.37)))))))))))))
