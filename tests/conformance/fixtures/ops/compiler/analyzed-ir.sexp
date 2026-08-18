(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (casts . %casts.11) (curve_ops . %curve_ops.12)
   (field_arith . %field_arith.9)
   (field_reduce . %field_reduce.10) (hits . %hits.7)
   (ledger_ops . %ledger_ops.8)
   (persistent_hashes . %persistent_hashes.5)
   (scores . %scores.6) (scratch . %scratch.3) (seen . %seen.4)
   (tag_cell . %tag_cell.1)
   (transient_conversions . %transient_conversions.2)
   (transient_hashes . %transient_hashes.0))
 (contract-types)
 (kernel-declaration (%kernel.77 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%scratch.3
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-native))))
     (%tag_cell.1 (1) (exported #t) (__compact_Cell (tbytes 32)))
     (%seen.4 (2) (exported #t) (Set (tbytes 32)))
     (%scores.6
       (3)
       (exported #t)
       (Map (tfield (field-native))
            (tunsigned 18446744073709551615)))
     (%hits.7 (4) (exported #t) (Counter)))
   (constructor () (tuple)))
 (native
   %transientHash.45
   (entry "__compactRuntime.transientHash" circuit)
   ((%value.74 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native
   %transientCommit.47
   (entry "__compactRuntime.transientCommit" circuit)
   ((%value.75 (tfield (field-native)))
     (%rand.76 (tfield (field-native))))
   (tfield (field-native)))
 (native
   %persistentHash.50
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.71 (tvector 2 (tbytes 32))))
   (tbytes 32))
 (native
   %persistentCommit.52
   (entry "__compactRuntime.persistentCommit" circuit)
   ((%value.72 (tbytes 32)) (%rand.73 (tbytes 32)))
   (tbytes 32))
 (native
   %degradeToTransient.24
   (entry "__compactRuntime.degradeToTransient" circuit)
   ((%x.69 (tbytes 32)))
   (tfield (field-native)))
 (native
   %upgradeFromTransient.26
   (entry "__compactRuntime.upgradeFromTransient" circuit)
   ((%x.70 (tfield (field-native))))
   (tbytes 32))
 (native
   %jubjubPointX.40
   (entry "__compactRuntime.jubjubPointX" circuit)
   ((%pt.67 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native
   %jubjubPointY.41
   (entry "__compactRuntime.jubjubPointY" circuit)
   ((%pt.68 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native
   %ecAdd.36
   (entry "__compactRuntime.ecAdd" circuit)
   ((%a.63 (tpoint (curve-jubjub)))
     (%b.64 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native
   %ecMul.38
   (entry "__compactRuntime.ecMul" circuit)
   ((%a.65 (tpoint (curve-jubjub)))
     (%b.66 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native
   %ecMulGenerator.32
   (entry "__compactRuntime.ecMulGenerator" circuit)
   ((%b.61 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native
   %hashToCurve.34
   (entry "__compactRuntime.hashToCurve" circuit)
   ((%value.62 (tvector 2 (tfield (field-native)))))
   (tpoint (curve-jubjub)))
 (circuit %field_arith.9 (exported #t) (pure #f) (proof #t)
   ((%a.53 (tfield (field-native)))
     (%b.54 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%p.55 (tfield (field-native))) (* (tfield
                                                (field-native))
                                              (var-ref %a.53)
                                              (var-ref %b.54))))
     (let* (((%s.56 (tfield (field-native))) (+ (tfield
                                                  (field-native))
                                                (var-ref %p.55)
                                                (var-ref %a.53))))
       (let* (((%d.57 (tfield (field-native))) (- (tfield
                                                    (field-native))
                                                  (var-ref %s.56)
                                                  (var-ref %b.54))))
         (seq (public-ledger %scratch.3 (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %d.57))))
                  (ins (cached #f) (n 1)))
                (var-ref %d.57))
              (return (var-ref %d.57)))))))
 (circuit %field_reduce.10 (exported #t) (pure #f) (proof #t)
   ((%c.58 (tfield (field-native)))
     (%q.59 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%r.60 (tfield (field-native))) (- (tfield
                                                (field-native))
                                              (var-ref %c.58)
                                              (* (tfield (field-native))
                                                 (var-ref %q.59)
                                                 '6554484396890773809930967563523245729705921265872317281365359162392183254199))))
     (seq (public-ledger %scratch.3 (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %r.60))))
              (ins (cached #f) (n 1)))
            (var-ref %r.60))
          (return (var-ref %r.60)))))
 (circuit %transient_hashes.0 (exported #t) (pure #f) (proof #t)
   ((%x.42 (tfield (field-native)))
     (%r.43 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%h.44 (tfield (field-native))) (call
                                             %transientHash.45
                                             (tuple
                                               (single (var-ref %x.42))
                                               (single (var-ref %r.43))))))
     (let* (((%c.46 (tfield (field-native))) (call
                                               %transientCommit.47
                                               (var-ref %h.44)
                                               (var-ref %r.43))))
       (seq (public-ledger %scratch.3 (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.46))))
                (ins (cached #f) (n 1)))
              (var-ref %c.46))
            (return (var-ref %c.46))))))
 (circuit %persistent_hashes.5 (exported #t) (pure #f) (proof #t)
   ((%x.48 (tbytes 32))) (tbytes 32)
   (let* (((%h.49 (tbytes 32)) (call
                                 %persistentHash.50
                                 (tuple
                                   (single
                                     '#vu8(111 112 115 58 112 104 58 0 0 0
                                           0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
                                           0 0 0 0 0 0))
                                   (single (var-ref %x.48))))))
     (let* (((%c.51 (tbytes 32)) (call
                                   %persistentCommit.52
                                   (var-ref %h.49)
                                   (var-ref %x.48))))
       (seq (public-ledger %tag_cell.1 (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.51))))
                (ins (cached #f) (n 1)))
              (var-ref %c.51))
            (return (var-ref %c.51))))))
 (circuit %transient_conversions.2 (exported #t) (pure #f) (proof #t)
   ((%x.22 (tbytes 32))) (tfield (field-native))
   (let* (((%f.23 (tfield (field-native))) (call
                                             %degradeToTransient.24
                                             (var-ref %x.22))))
     (let* (((%up.25 (tbytes 32)) (call
                                    %upgradeFromTransient.26
                                    (var-ref %f.23))))
       (let* (((%f2.27 (tfield (field-native))) (call
                                                  %degradeToTransient.24
                                                  (var-ref %up.25))))
         (seq (let* (((%tmp.28 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (var-ref %f.23)
                                                           (var-ref
                                                             %f2.27))))
                (public-ledger %scratch.3 (0) write (ttuple)
                  (instructions
                    (push
                      (storage #f)
                      (value (state-value cell (align 0 1))))
                    (push
                      (storage #t)
                      (value (state-value cell (var-ref %tmp.28))))
                    (ins (cached #f) (n 1)))
                  (var-ref %tmp.28)))
              (return (var-ref %f2.27)))))))
 (circuit %curve_ops.12 (exported #t) (pure #f) (proof #t)
   ((%s.29 (tfield (field-native)))
     (%m.30 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%g.31 (tpoint (curve-jubjub))) (call
                                             %ecMulGenerator.32
                                             (cast-to-field
                                               (field-scalar
                                                 (curve-jubjub))
                                               (tfield (field-native))
                                               (var-ref %s.29)))))
     (let* (((%h.33 (tpoint (curve-jubjub))) (call
                                               %hashToCurve.34
                                               (tuple
                                                 (single (var-ref %s.29))
                                                 (single
                                                   (var-ref %m.30))))))
       (let* (((%sum.35 (tpoint (curve-jubjub))) (call
                                                   %ecAdd.36
                                                   (var-ref %g.31)
                                                   (var-ref %h.33))))
         (let* (((%prod.37 (tpoint (curve-jubjub))) (call
                                                      %ecMul.38
                                                      (var-ref %sum.35)
                                                      (cast-to-field
                                                        (field-scalar
                                                          (curve-jubjub))
                                                        (tfield
                                                          (field-native))
                                                        (var-ref %m.30)))))
           (let* (((%packed.39 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (call
                                                             %jubjubPointX.40
                                                             (var-ref
                                                               %prod.37))
                                                           (call
                                                             %jubjubPointY.41
                                                             (var-ref
                                                               %prod.37)))))
             (seq (public-ledger %scratch.3 (0) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 0 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %packed.39))))
                      (ins (cached #f) (n 1)))
                    (var-ref %packed.39))
                  (return (var-ref %packed.39)))))))))
 (circuit %ledger_ops.8 (exported #t) (pure #f) (proof #t)
   ((%k.14 (tfield (field-native)))
     (%v.15 (tunsigned 18446744073709551615))
     (%entry.13 (tbytes 32)))
   (tboolean)
   (seq (let* (((%tmp.16 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 1)
                                              '1)))
          (public-ledger %hits.7 (4) increment (ttuple)
            (instructions
              (idx (cached #f) (pushPath #t) (path ((align 4 1))))
              (addi (immediate (value->int (var-ref %tmp.16))))
              (ins (cached #t) (n 1)))
            (var-ref %tmp.16)))
        (public-ledger %scores.6 (3) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 3 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %k.14))))
            (push
              (storage #t)
              (value
                (state-value
                  ADT
                  (var-ref %v.15)
                  (tunsigned 18446744073709551615))))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %k.14) (var-ref %v.15))
        (public-ledger %seen.4 (2) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %entry.13))))
            (push (storage #t) (value (state-value null)))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %entry.13))
        (return
          (public-ledger %seen.4 (2) member (tboolean)
            (instructions (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %entry.13))))
              (member) (popeq (cached #t) (result (void))))
            (var-ref %entry.13)))))
 (circuit %casts.11 (exported #t) (pure #f) (proof #t)
   ((%n.17 (tunsigned 4294967295))
     (%f.18 (tfield (field-native))))
   (tbytes 32)
   (let* (((%wide.19 (tunsigned 36893488147419103230)) (+ (tunsigned
                                                            36893488147419103230)
                                                          (safe-cast
                                                            (tunsigned
                                                              36893488147419103230)
                                                            (tunsigned
                                                              18446744073709551615)
                                                            (safe-cast
                                                              (tunsigned
                                                                18446744073709551615)
                                                              (tunsigned
                                                                4294967295)
                                                              (var-ref
                                                                %n.17)))
                                                          (safe-cast
                                                            (tunsigned
                                                              36893488147419103230)
                                                            (tunsigned
                                                              18446744073709551615)
                                                            (safe-cast
                                                              (tunsigned
                                                                18446744073709551615)
                                                              (tunsigned 1)
                                                              '1)))))
     (let* (((%as_field.20 (tfield (field-native))) (safe-cast
                                                      (tfield
                                                        (field-native))
                                                      (tunsigned
                                                        36893488147419103230)
                                                      (var-ref %wide.19))))
       (let* (((%b.21 (tbytes 32)) (field->bytes
                                     32
                                     (field-native)
                                     (+ (tfield (field-native))
                                        (var-ref %f.18)
                                        (var-ref %as_field.20)))))
         (seq (public-ledger %tag_cell.1 (1) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 1 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %b.21))))
                  (ins (cached #f) (n 1)))
                (var-ref %b.21))
              (return (var-ref %b.21))))))))
