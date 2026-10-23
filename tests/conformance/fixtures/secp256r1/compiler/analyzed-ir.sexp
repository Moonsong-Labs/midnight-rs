(analyzed-ir (compiler-version "0.35.103-dev")
 (language-version "0.27.0") (runtime-version "0.20.101")
 (exports (base_cell . %base_cell.66) (base_ops . %base_ops.67)
   (casts . %casts.64) (curve_ops . %curve_ops.65)
   (point_cell . %point_cell.62)
   (scalar_cell . %scalar_cell.63)
   (scalar_ops . %scalar_ops.60) (verified . %verified.61)
   (verify . %verify.59))
 (contract-types)
 (kernel-declaration (%kernel.110 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%base_cell.66
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-base (curve-secp256r1)))))
     (%scalar_cell.63
       (1)
       (exported #t)
       (__compact_Cell (tfield (field-scalar (curve-secp256r1)))))
     (%point_cell.62
       (2)
       (exported #t)
       (__compact_Cell (tpoint (curve-secp256r1))))
     (%verified.61
       (3)
       (exported #t)
       (__compact_Cell (tboolean))))
   (constructor () (tuple)))
 (circuit %reverseBytes32.96 (exported #f) (pure #t) (proof #f)
   ((%b.97 (tbytes 32))) (tbytes 32)
   (let* (((%v.98 (tvector 32 (tunsigned 255))) (bytes->vector
                                                  32
                                                  (var-ref %b.97))))
     (return
       (vector->bytes
         32
         (vector (single (tuple-ref (var-ref %v.98) 31))
          (single (tuple-ref (var-ref %v.98) 30))
          (single (tuple-ref (var-ref %v.98) 29))
          (single (tuple-ref (var-ref %v.98) 28))
          (single (tuple-ref (var-ref %v.98) 27))
          (single (tuple-ref (var-ref %v.98) 26))
          (single (tuple-ref (var-ref %v.98) 25))
          (single (tuple-ref (var-ref %v.98) 24))
          (single (tuple-ref (var-ref %v.98) 23))
          (single (tuple-ref (var-ref %v.98) 22))
          (single (tuple-ref (var-ref %v.98) 21))
          (single (tuple-ref (var-ref %v.98) 20))
          (single (tuple-ref (var-ref %v.98) 19))
          (single (tuple-ref (var-ref %v.98) 18))
          (single (tuple-ref (var-ref %v.98) 17))
          (single (tuple-ref (var-ref %v.98) 16))
          (single (tuple-ref (var-ref %v.98) 15))
          (single (tuple-ref (var-ref %v.98) 14))
          (single (tuple-ref (var-ref %v.98) 13))
          (single (tuple-ref (var-ref %v.98) 12))
          (single (tuple-ref (var-ref %v.98) 11))
          (single (tuple-ref (var-ref %v.98) 10))
          (single (tuple-ref (var-ref %v.98) 9))
          (single (tuple-ref (var-ref %v.98) 8))
          (single (tuple-ref (var-ref %v.98) 7))
          (single (tuple-ref (var-ref %v.98) 6))
          (single (tuple-ref (var-ref %v.98) 5))
          (single (tuple-ref (var-ref %v.98) 4))
          (single (tuple-ref (var-ref %v.98) 3))
          (single (tuple-ref (var-ref %v.98) 2))
          (single (tuple-ref (var-ref %v.98) 1))
          (single (tuple-ref (var-ref %v.98) 0)))))))
 (circuit %secp256r1EcdsaVerify.68 (exported #f) (pure #t) (proof #f)
   ((%msgHash.99 (tbytes 32))
     (%sig.100
       (tstruct
         Secp256r1EcdsaSignature
         (r (tfield (field-scalar (curve-secp256r1))))
         (s (tfield (field-scalar (curve-secp256r1))))))
     (%pk.107 (tpoint (curve-secp256r1))))
   (tboolean)
   (seq (assert
          (!= (tpoint (curve-secp256r1))
              (var-ref %pk.107)
              (default (tpoint (curve-secp256r1))))
          "Secp256r1Point identity is not a permitted secp256r1EcdsaVerify verification key")
        (let* (((%z.103 (tfield (field-scalar (curve-secp256r1)))) (cast-from-bytes
                                                                     (tfield
                                                                       (field-scalar
                                                                         (curve-secp256r1)))
                                                                     32
                                                                     (call
                                                                       %reverseBytes32.96
                                                                       (var-ref
                                                                         %msgHash.99)))))
          (let* (((%__compact_pattern_tmp2.101
                    (tstruct
                      Secp256r1EcdsaSignature
                      (r (tfield (field-scalar (curve-secp256r1))))
                      (s (tfield (field-scalar (curve-secp256r1)))))) (var-ref
                                                                        %sig.100)))
            (let* (((%r.105 (tfield (field-scalar (curve-secp256r1)))) (elt-ref
                                                                         (var-ref
                                                                           %__compact_pattern_tmp2.101)
                                                                         r
                                                                         0)))
              (let* (((%s.102 (tfield (field-scalar (curve-secp256r1)))) (elt-ref
                                                                           (var-ref
                                                                             %__compact_pattern_tmp2.101)
                                                                           s
                                                                           1)))
                (let* (((%w.104 (tfield (field-scalar (curve-secp256r1)))) (call
                                                                             %inv.90
                                                                             (var-ref
                                                                               %s.102))))
                  (let* (((%u1.106
                            (tfield (field-scalar (curve-secp256r1)))) (* (tfield
                                                                            (field-scalar
                                                                              (curve-secp256r1)))
                                                                          (var-ref
                                                                            %z.103)
                                                                          (var-ref
                                                                            %w.104))))
                    (let* (((%u2.108
                              (tfield (field-scalar (curve-secp256r1)))) (* (tfield
                                                                              (field-scalar
                                                                                (curve-secp256r1)))
                                                                            (var-ref
                                                                              %r.105)
                                                                            (var-ref
                                                                              %w.104))))
                      (let* (((%point.109 (tpoint (curve-secp256r1))) (call
                                                                        %ecAdd.73
                                                                        (call
                                                                          %ecMulGenerator.74
                                                                          (var-ref
                                                                            %u1.106))
                                                                        (call
                                                                          %ecMul.76
                                                                          (var-ref
                                                                            %pk.107)
                                                                          (var-ref
                                                                            %u2.108)))))
                        (return
                          (== (tfield (field-scalar (curve-secp256r1)))
                              (cast-from-bytes
                                (tfield (field-scalar (curve-secp256r1)))
                                32
                                (field->bytes
                                  32
                                  (field-base (curve-secp256r1))
                                  (call
                                    %secp256r1PointX.79
                                    (var-ref %point.109))))
                              (var-ref %r.105)))))))))))))
 (native %neg.87 (entry "__compactRuntime.secp256r1BaseNeg" circuit)
   (type-arguments)
   ((%s.111 (tfield (field-base (curve-secp256r1)))))
   (tfield (field-base (curve-secp256r1))))
 (native %inv.86 (entry "__compactRuntime.secp256r1BaseInv" circuit)
   (type-arguments)
   ((%s.112 (tfield (field-base (curve-secp256r1)))))
   (tfield (field-base (curve-secp256r1))))
 (native %neg.93
   (entry "__compactRuntime.secp256r1ScalarNeg" circuit)
   (type-arguments)
   ((%s.113 (tfield (field-scalar (curve-secp256r1)))))
   (tfield (field-scalar (curve-secp256r1))))
 (native %inv.90
   (entry "__compactRuntime.secp256r1ScalarInv" circuit)
   (type-arguments)
   ((%s.114 (tfield (field-scalar (curve-secp256r1)))))
   (tfield (field-scalar (curve-secp256r1))))
 (native %secp256r1PointX.79
   (entry "__compactRuntime.secp256r1PointX" circuit)
   (type-arguments) ((%pt.115 (tpoint (curve-secp256r1))))
   (tfield (field-base (curve-secp256r1))))
 (native %secp256r1PointY.80
   (entry "__compactRuntime.secp256r1PointY" circuit)
   (type-arguments) ((%pt.116 (tpoint (curve-secp256r1))))
   (tfield (field-base (curve-secp256r1))))
 (native %ecAdd.73 (entry "__compactRuntime.secp256r1Add" circuit)
   (type-arguments)
   ((%a.117 (tpoint (curve-secp256r1)))
     (%b.118 (tpoint (curve-secp256r1))))
   (tpoint (curve-secp256r1)))
 (native %ecMul.76 (entry "__compactRuntime.secp256r1Mul" circuit)
   (type-arguments)
   ((%a.119 (tpoint (curve-secp256r1)))
     (%b.120 (tfield (field-scalar (curve-secp256r1)))))
   (tpoint (curve-secp256r1)))
 (native %ecMulGenerator.74
   (entry "__compactRuntime.secp256r1MulGenerator" circuit)
   (type-arguments)
   ((%b.121 (tfield (field-scalar (curve-secp256r1)))))
   (tpoint (curve-secp256r1)))
 (witness
   %secret_scalar.89
   ()
   (tfield (field-scalar (curve-secp256r1))))
 (circuit %base_ops.67 (exported #t) (pure #f) (proof #t)
   ((%a.84 (tfield (field-base (curve-secp256r1))))
     (%b.85 (tfield (field-base (curve-secp256r1)))))
   (tvector 5 (tfield (field-base (curve-secp256r1))))
   (let* (((%sum.88 (tfield (field-base (curve-secp256r1)))) (+ (tfield
                                                                  (field-base
                                                                    (curve-secp256r1)))
                                                                (var-ref
                                                                  %a.84)
                                                                (var-ref
                                                                  %b.85))))
     (seq (public-ledger %base_cell.66 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %sum.88))))
              (ins (cached #f) (n 1)))
            (var-ref %sum.88))
          (return
            (tuple (single (var-ref %sum.88))
              (single
                (- (tfield (field-base (curve-secp256r1)))
                   (var-ref %a.84)
                   (var-ref %b.85)))
              (single
                (* (tfield (field-base (curve-secp256r1)))
                   (var-ref %a.84)
                   (var-ref %b.85)))
              (single (call %neg.87 (var-ref %a.84)))
              (single (call %inv.86 (var-ref %b.85))))))))
 (circuit %scalar_ops.60 (exported #t) (pure #f) (proof #t)
   ((%a.92 (tfield (field-scalar (curve-secp256r1))))
     (%b.91 (tfield (field-scalar (curve-secp256r1)))))
   (tvector 5 (tfield (field-scalar (curve-secp256r1))))
   (let* (((%k.94 (tfield (field-scalar (curve-secp256r1)))) (call
                                                               %secret_scalar.89)))
     (seq (let* (((%tmp.95
                    (tfield (field-scalar (curve-secp256r1)))) (* (tfield
                                                                    (field-scalar
                                                                      (curve-secp256r1)))
                                                                  (var-ref
                                                                    %a.92)
                                                                  (var-ref
                                                                    %k.94))))
            (public-ledger %scalar_cell.63 write (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %tmp.95))))
                (ins (cached #f) (n 1)))
              (var-ref %tmp.95)))
          (return
            (tuple
              (single
                (+ (tfield (field-scalar (curve-secp256r1)))
                   (var-ref %a.92)
                   (var-ref %b.91)))
              (single
                (- (tfield (field-scalar (curve-secp256r1)))
                   (var-ref %a.92)
                   (var-ref %b.91)))
              (single
                (* (tfield (field-scalar (curve-secp256r1)))
                   (var-ref %a.92)
                   (var-ref %b.91)))
              (single (call %neg.93 (var-ref %a.92)))
              (single (call %inv.90 (var-ref %b.91))))))))
 (circuit %curve_ops.65 (exported #t) (pure #f) (proof #t)
   ((%p.77 (tpoint (curve-secp256r1)))
     (%k.75 (tfield (field-scalar (curve-secp256r1)))))
   (ttuple
     (tfield (field-base (curve-secp256r1)))
     (tfield (field-base (curve-secp256r1)))
     (tboolean)
     (tboolean))
   (let* (((%q.78 (tpoint (curve-secp256r1))) (call
                                                %ecAdd.73
                                                (call
                                                  %ecMulGenerator.74
                                                  (var-ref %k.75))
                                                (call
                                                  %ecMul.76
                                                  (var-ref %p.77)
                                                  (var-ref %k.75)))))
     (seq (public-ledger %point_cell.62 write (2) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 2 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %q.78))))
              (ins (cached #f) (n 1)))
            (var-ref %q.78))
          (return
            (tuple
              (single (call %secp256r1PointX.79 (var-ref %q.78)))
              (single (call %secp256r1PointY.80 (var-ref %q.78)))
              (single
                (== (tpoint (curve-secp256r1))
                    (var-ref %q.78)
                    (var-ref %p.77)))
              (single
                (== (tpoint (curve-secp256r1))
                    (call
                      %ecMulGenerator.74
                      (default (tfield (field-scalar (curve-secp256r1)))))
                    (default (tpoint (curve-secp256r1))))))))))
 (circuit %casts.64 (exported #t) (pure #f) (proof #t)
   ((%b.81 (tbytes 32))
     (%x.82 (tfield (field-base (curve-secp256r1)))))
   (ttuple
     (tfield (field-scalar (curve-secp256r1)))
     (tfield (field-base (curve-secp256r1)))
     (tbytes 32))
   (let* (((%s.83 (tfield (field-scalar (curve-secp256r1)))) (cast-from-bytes
                                                               (tfield
                                                                 (field-scalar
                                                                   (curve-secp256r1)))
                                                               32
                                                               (var-ref
                                                                 %b.81))))
     (seq (public-ledger %scalar_cell.63 write (1) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 1 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %s.83))))
              (ins (cached #f) (n 1)))
            (var-ref %s.83))
          (return
            (tuple
              (single (var-ref %s.83))
              (single
                (cast-from-bytes
                  (tfield (field-base (curve-secp256r1)))
                  32
                  (var-ref %b.81)))
              (single
                (field->bytes
                  32
                  (field-base (curve-secp256r1))
                  (var-ref %x.82))))))))
 (circuit %verify.59 (exported #t) (pure #f) (proof #t)
   ((%msg_hash.70 (tbytes 32))
     (%sig.71
       (tstruct
         Secp256r1EcdsaSignature
         (r (tfield (field-scalar (curve-secp256r1))))
         (s (tfield (field-scalar (curve-secp256r1))))))
     (%pk.69 (tpoint (curve-secp256r1))))
   (tboolean)
   (let* (((%valid.72 (tboolean)) (call
                                    %secp256r1EcdsaVerify.68
                                    (var-ref %msg_hash.70)
                                    (var-ref %sig.71)
                                    (var-ref %pk.69))))
     (seq (public-ledger %verified.61 write (3) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 3 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %valid.72))))
              (ins (cached #f) (n 1)))
            (var-ref %valid.72))
          (return (var-ref %valid.72))))))
