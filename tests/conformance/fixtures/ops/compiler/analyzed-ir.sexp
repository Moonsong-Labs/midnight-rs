(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (casts . %casts.28) (curve_ops . %curve_ops.29)
   (field_arith . %field_arith.26)
   (field_reduce . %field_reduce.27) (hits . %hits.24)
   (ledger_ops . %ledger_ops.25)
   (persistent_hashes . %persistent_hashes.22)
   (scores . %scores.23) (scratch . %scratch.20)
   (seen . %seen.21) (tag_cell . %tag_cell.18)
   (transient_conversions . %transient_conversions.19)
   (transient_hashes . %transient_hashes.17))
 (contract-types)
 (kernel-declaration (%kernel.75 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%scratch.20
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-native))))
     (%tag_cell.18
       (1)
       (exported #t)
       (__compact_Cell (tbytes 32)))
     (%seen.21 (2) (exported #t) (Set (tbytes 32)))
     (%scores.23
       (3)
       (exported #t)
       (Map (tfield (field-native))
            (tunsigned 18446744073709551615)))
     (%hits.24 (4) (exported #t) (Counter)))
   (constructor () (tuple)))
 (native %transientHash.58
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.76 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %transientCommit.61
   (entry "__compactRuntime.transientCommit" circuit)
   (type-arguments (tfield (field-native)))
   ((%value.77 (tfield (field-native)))
     (%rand.78 (tfield (field-native))))
   (tfield (field-native)))
 (native %persistentHash.64
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 2 (tbytes 32)))
   ((%value.79 (tvector 2 (tbytes 32)))) (tbytes 32))
 (native %persistentCommit.66
   (entry "__compactRuntime.persistentCommit" circuit)
   (type-arguments (tbytes 32))
   ((%value.80 (tbytes 32)) (%rand.81 (tbytes 32)))
   (tbytes 32))
 (native %degradeToTransient.38
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.82 (tbytes 32)))
   (tfield (field-native)))
 (native %upgradeFromTransient.40
   (entry "__compactRuntime.upgradeFromTransient" circuit)
   (type-arguments) ((%x.83 (tfield (field-native))))
   (tbytes 32))
 (native %jubjubPointX.54
   (entry "__compactRuntime.jubjubPointX" circuit)
   (type-arguments) ((%pt.84 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %jubjubPointY.56
   (entry "__compactRuntime.jubjubPointY" circuit)
   (type-arguments) ((%pt.85 (tpoint (curve-jubjub))))
   (tfield (field-native)))
 (native %ecAdd.49 (entry "__compactRuntime.ecAdd" circuit)
   (type-arguments)
   ((%a.86 (tpoint (curve-jubjub)))
     (%b.87 (tpoint (curve-jubjub))))
   (tpoint (curve-jubjub)))
 (native %ecMul.52 (entry "__compactRuntime.ecMul" circuit)
   (type-arguments)
   ((%a.88 (tpoint (curve-jubjub)))
     (%b.89 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %ecMulGenerator.45
   (entry "__compactRuntime.ecMulGenerator" circuit)
   (type-arguments)
   ((%b.90 (tfield (field-scalar (curve-jubjub)))))
   (tpoint (curve-jubjub)))
 (native %hashToCurve.47
   (entry "__compactRuntime.hashToCurve" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.91 (tvector 2 (tfield (field-native)))))
   (tpoint (curve-jubjub)))
 (circuit %field_arith.26 (exported #t) (pure #f) (proof #t)
   ((%a.69 (tfield (field-native)))
     (%b.70 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%p.12 (tfield (field-native))) (* (tfield
                                                (field-native))
                                              (var-ref %a.69)
                                              (var-ref %b.70))))
     (let* (((%s.13 (tfield (field-native))) (+ (tfield
                                                  (field-native))
                                                (var-ref %p.12)
                                                (var-ref %a.69))))
       (let* (((%d.71 (tfield (field-native))) (- (tfield
                                                    (field-native))
                                                  (var-ref %s.13)
                                                  (var-ref %b.70))))
         (seq (public-ledger %scratch.20 write (0) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 0 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %d.71))))
                  (ins (cached #f) (n 1)))
                (var-ref %d.71))
              (return (var-ref %d.71)))))))
 (circuit %field_reduce.27 (exported #t) (pure #f) (proof #t)
   ((%c.72 (tfield (field-native)))
     (%q.73 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%r.74 (tfield (field-native))) (- (tfield
                                                (field-native))
                                              (var-ref %c.72)
                                              (* (tfield (field-native))
                                                 (var-ref %q.73)
                                                 '6554484396890773809930967563523245729705921265872317281365359162392183254199))))
     (seq (public-ledger %scratch.20 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %r.74))))
              (ins (cached #f) (n 1)))
            (var-ref %r.74))
          (return (var-ref %r.74)))))
 (circuit %transient_hashes.17 (exported #t) (pure #f) (proof #t)
   ((%x.59 (tfield (field-native)))
     (%r.60 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%h.62 (tfield (field-native))) (call
                                             %transientHash.58
                                             (tuple
                                               (single (var-ref %x.59))
                                               (single (var-ref %r.60))))))
     (let* (((%c.63 (tfield (field-native))) (call
                                               %transientCommit.61
                                               (var-ref %h.62)
                                               (var-ref %r.60))))
       (seq (public-ledger %scratch.20 write (0) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 0 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.63))))
                (ins (cached #f) (n 1)))
              (var-ref %c.63))
            (return (var-ref %c.63))))))
 (circuit %persistent_hashes.22 (exported #t) (pure #f) (proof #t)
   ((%x.65 (tbytes 32))) (tbytes 32)
   (let* (((%h.67 (tbytes 32)) (call
                                 %persistentHash.64
                                 (tuple
                                   (single
                                     '#vu8(111 112 115 58 112 104 58 0 0 0
                                           0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
                                           0 0 0 0 0 0))
                                   (single (var-ref %x.65))))))
     (let* (((%c.68 (tbytes 32)) (call
                                   %persistentCommit.66
                                   (var-ref %h.67)
                                   (var-ref %x.65))))
       (seq (public-ledger %tag_cell.18 write (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %c.68))))
                (ins (cached #f) (n 1)))
              (var-ref %c.68))
            (return (var-ref %c.68))))))
 (circuit %transient_conversions.19 (exported #t) (pure #f)
   (proof #t) ((%x.39 (tbytes 32))) (tfield (field-native))
   (let* (((%f.41 (tfield (field-native))) (call
                                             %degradeToTransient.38
                                             (var-ref %x.39))))
     (let* (((%up.42 (tbytes 32)) (call
                                    %upgradeFromTransient.40
                                    (var-ref %f.41))))
       (let* (((%f2.43 (tfield (field-native))) (call
                                                  %degradeToTransient.38
                                                  (var-ref %up.42))))
         (seq (let* (((%tmp.44 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (var-ref %f.41)
                                                           (var-ref
                                                             %f2.43))))
                (public-ledger %scratch.20 write (0) write (ttuple)
                  (instructions
                    (push
                      (storage #f)
                      (value (state-value cell (align 0 1))))
                    (push
                      (storage #t)
                      (value (state-value cell (var-ref %tmp.44))))
                    (ins (cached #f) (n 1)))
                  (var-ref %tmp.44)))
              (return (var-ref %f2.43)))))))
 (circuit %curve_ops.29 (exported #t) (pure #f) (proof #t)
   ((%s.46 (tfield (field-native)))
     (%m.48 (tfield (field-native))))
   (tfield (field-native))
   (let* (((%g.50 (tpoint (curve-jubjub))) (call
                                             %ecMulGenerator.45
                                             (cast-to-field
                                               (field-scalar
                                                 (curve-jubjub))
                                               (tfield (field-native))
                                               (var-ref %s.46)))))
     (let* (((%h.51 (tpoint (curve-jubjub))) (call
                                               %hashToCurve.47
                                               (tuple
                                                 (single (var-ref %s.46))
                                                 (single
                                                   (var-ref %m.48))))))
       (let* (((%sum.53 (tpoint (curve-jubjub))) (call
                                                   %ecAdd.49
                                                   (var-ref %g.50)
                                                   (var-ref %h.51))))
         (let* (((%prod.55 (tpoint (curve-jubjub))) (call
                                                      %ecMul.52
                                                      (var-ref %sum.53)
                                                      (cast-to-field
                                                        (field-scalar
                                                          (curve-jubjub))
                                                        (tfield
                                                          (field-native))
                                                        (var-ref %m.48)))))
           (let* (((%packed.57 (tfield (field-native))) (+ (tfield
                                                             (field-native))
                                                           (call
                                                             %jubjubPointX.54
                                                             (var-ref
                                                               %prod.55))
                                                           (call
                                                             %jubjubPointY.56
                                                             (var-ref
                                                               %prod.55)))))
             (seq (public-ledger %scratch.20 write (0) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 0 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %packed.57))))
                      (ins (cached #f) (n 1)))
                    (var-ref %packed.57))
                  (return (var-ref %packed.57)))))))))
 (circuit %ledger_ops.25 (exported #t) (pure #f) (proof #t)
   ((%k.32 (tfield (field-native)))
     (%v.33 (tunsigned 18446744073709551615))
     (%entry.30 (tbytes 32)))
   (tboolean)
   (seq (let* (((%tmp.31 (tunsigned 65535)) (safe-cast
                                              (tunsigned 65535)
                                              (tunsigned 1)
                                              '1)))
          (public-ledger %hits.24 update (4) increment (ttuple)
            (instructions
              (idx (cached #f) (pushPath #t) (path ((align 4 1))))
              (addi (immediate (value->int (var-ref %tmp.31))))
              (ins (cached #t) (n 1)))
            (var-ref %tmp.31)))
        (public-ledger %scores.23 update (3) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 3 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %k.32))))
            (push
              (storage #t)
              (value
                (state-value
                  ADT
                  (var-ref %v.33)
                  (tunsigned 18446744073709551615))))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %k.32) (var-ref %v.33))
        (public-ledger %seen.21 update (2) insert (ttuple)
          (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
            (push
              (storage #f)
              (value (state-value cell (var-ref %entry.30))))
            (push (storage #t) (value (state-value null)))
            (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
          (var-ref %entry.30))
        (return
          (public-ledger %seen.21 read (2) member (tboolean)
            (instructions (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %entry.30))))
              (member) (popeq (cached #t) (result (void))))
            (var-ref %entry.30)))))
 (circuit %casts.28 (exported #t) (pure #f) (proof #t)
   ((%n.34 (tunsigned 4294967295))
     (%f.35 (tfield (field-native))))
   (tbytes 32)
   (let* (((%wide.1 (tunsigned 36893488147419103230)) (+ (tunsigned
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
                                                               %n.34)))
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
     (let* (((%as_field.36 (tfield (field-native))) (safe-cast
                                                      (tfield
                                                        (field-native))
                                                      (tunsigned
                                                        36893488147419103230)
                                                      (var-ref %wide.1))))
       (let* (((%b.37 (tbytes 32)) (field->bytes
                                     32
                                     (field-native)
                                     (+ (tfield (field-native))
                                        (var-ref %f.35)
                                        (var-ref %as_field.36)))))
         (seq (public-ledger %tag_cell.18 write (1) write (ttuple)
                (instructions
                  (push
                    (storage #f)
                    (value (state-value cell (align 1 1))))
                  (push
                    (storage #t)
                    (value (state-value cell (var-ref %b.37))))
                  (ins (cached #f) (n 1)))
                (var-ref %b.37))
              (return (var-ref %b.37))))))))
