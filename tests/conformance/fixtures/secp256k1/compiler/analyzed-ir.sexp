(analyzed-ir (compiler-version "0.35.103-dev")
 (language-version "0.27.0") (runtime-version "0.20.101")
 (exports (address . %address.100) (base_cell . %base_cell.101)
   (base_ops . %base_ops.98) (casts . %casts.99)
   (curve_ops . %curve_ops.96) (eth_address . %eth_address.97)
   (point_cell . %point_cell.94)
   (scalar_cell . %scalar_cell.95)
   (scalar_ops . %scalar_ops.92) (verified . %verified.93)
   (verify . %verify.91))
 (contract-types)
 (kernel-declaration (%kernel.153 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%base_cell.101
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-base (curve-secp256k1)))))
     (%scalar_cell.95
       (1)
       (exported #t)
       (__compact_Cell (tfield (field-scalar (curve-secp256k1)))))
     (%point_cell.94
       (2)
       (exported #t)
       (__compact_Cell (tpoint (curve-secp256k1))))
     (%verified.93 (3) (exported #t) (__compact_Cell (tboolean)))
     (%address.100
       (4)
       (exported #t)
       (__compact_Cell (tbytes 20))))
   (constructor () (tuple)))
 (circuit %reverseBytes32.139 (exported #f) (pure #t) (proof #f)
   ((%b.140 (tbytes 32))) (tbytes 32)
   (let* (((%v.141 (tvector 32 (tunsigned 255))) (bytes->vector
                                                   32
                                                   (var-ref %b.140))))
     (return
       (vector->bytes
         32
         (vector (single (tuple-ref (var-ref %v.141) 31))
          (single (tuple-ref (var-ref %v.141) 30))
          (single (tuple-ref (var-ref %v.141) 29))
          (single (tuple-ref (var-ref %v.141) 28))
          (single (tuple-ref (var-ref %v.141) 27))
          (single (tuple-ref (var-ref %v.141) 26))
          (single (tuple-ref (var-ref %v.141) 25))
          (single (tuple-ref (var-ref %v.141) 24))
          (single (tuple-ref (var-ref %v.141) 23))
          (single (tuple-ref (var-ref %v.141) 22))
          (single (tuple-ref (var-ref %v.141) 21))
          (single (tuple-ref (var-ref %v.141) 20))
          (single (tuple-ref (var-ref %v.141) 19))
          (single (tuple-ref (var-ref %v.141) 18))
          (single (tuple-ref (var-ref %v.141) 17))
          (single (tuple-ref (var-ref %v.141) 16))
          (single (tuple-ref (var-ref %v.141) 15))
          (single (tuple-ref (var-ref %v.141) 14))
          (single (tuple-ref (var-ref %v.141) 13))
          (single (tuple-ref (var-ref %v.141) 12))
          (single (tuple-ref (var-ref %v.141) 11))
          (single (tuple-ref (var-ref %v.141) 10))
          (single (tuple-ref (var-ref %v.141) 9))
          (single (tuple-ref (var-ref %v.141) 8))
          (single (tuple-ref (var-ref %v.141) 7))
          (single (tuple-ref (var-ref %v.141) 6))
          (single (tuple-ref (var-ref %v.141) 5))
          (single (tuple-ref (var-ref %v.141) 4))
          (single (tuple-ref (var-ref %v.141) 3))
          (single (tuple-ref (var-ref %v.141) 2))
          (single (tuple-ref (var-ref %v.141) 1))
          (single (tuple-ref (var-ref %v.141) 0)))))))
 (circuit %secp256k1EcdsaVerify.108 (exported #f) (pure #t)
   (proof #f)
   ((%msgHash.142 (tbytes 32))
     (%sig.143
       (tstruct
         Secp256k1EcdsaSignature
         (r (tfield (field-scalar (curve-secp256k1))))
         (s (tfield (field-scalar (curve-secp256k1))))))
     (%pk.150 (tpoint (curve-secp256k1))))
   (tboolean)
   (seq (assert
          (!= (tpoint (curve-secp256k1))
              (var-ref %pk.150)
              (default (tpoint (curve-secp256k1))))
          "Secp256k1Point identity is not a permitted secp256k1EcdsaVerify verification key")
        (let* (((%z.146 (tfield (field-scalar (curve-secp256k1)))) (cast-from-bytes
                                                                     (tfield
                                                                       (field-scalar
                                                                         (curve-secp256k1)))
                                                                     32
                                                                     (call
                                                                       %reverseBytes32.139
                                                                       (var-ref
                                                                         %msgHash.142)))))
          (let* (((%__compact_pattern_tmp3.144
                    (tstruct
                      Secp256k1EcdsaSignature
                      (r (tfield (field-scalar (curve-secp256k1))))
                      (s (tfield (field-scalar (curve-secp256k1)))))) (var-ref
                                                                        %sig.143)))
            (let* (((%r.148 (tfield (field-scalar (curve-secp256k1)))) (elt-ref
                                                                         (var-ref
                                                                           %__compact_pattern_tmp3.144)
                                                                         r
                                                                         0)))
              (let* (((%s.145 (tfield (field-scalar (curve-secp256k1)))) (elt-ref
                                                                           (var-ref
                                                                             %__compact_pattern_tmp3.144)
                                                                           s
                                                                           1)))
                (let* (((%w.147 (tfield (field-scalar (curve-secp256k1)))) (call
                                                                             %inv.114
                                                                             (var-ref
                                                                               %s.145))))
                  (let* (((%u1.149
                            (tfield (field-scalar (curve-secp256k1)))) (* (tfield
                                                                            (field-scalar
                                                                              (curve-secp256k1)))
                                                                          (var-ref
                                                                            %z.146)
                                                                          (var-ref
                                                                            %w.147))))
                    (let* (((%u2.151
                              (tfield (field-scalar (curve-secp256k1)))) (* (tfield
                                                                              (field-scalar
                                                                                (curve-secp256k1)))
                                                                            (var-ref
                                                                              %r.148)
                                                                            (var-ref
                                                                              %w.147))))
                      (let* (((%point.152 (tpoint (curve-secp256k1))) (call
                                                                        %ecAdd.120
                                                                        (call
                                                                          %ecMulGenerator.121
                                                                          (var-ref
                                                                            %u1.149))
                                                                        (call
                                                                          %ecMul.123
                                                                          (var-ref
                                                                            %pk.150)
                                                                          (var-ref
                                                                            %u2.151)))))
                        (return
                          (== (tfield (field-scalar (curve-secp256k1)))
                              (cast-from-bytes
                                (tfield (field-scalar (curve-secp256k1)))
                                32
                                (field->bytes
                                  32
                                  (field-base (curve-secp256k1))
                                  (call
                                    %secp256k1PointX.126
                                    (var-ref %point.152))))
                              (var-ref %r.148)))))))))))))
 (circuit %secp256k1BaseBigEndian.134 (exported #f) (pure #t)
   (proof #f)
   ((%b.135 (tfield (field-base (curve-secp256k1)))))
   (tvector 32 (tunsigned 255))
   (let* (((%vec.136 (tvector 32 (tunsigned 255))) (bytes->vector
                                                     32
                                                     (field->bytes
                                                       32
                                                       (field-base
                                                         (curve-secp256k1))
                                                       (var-ref %b.135)))))
     (return
       (tuple (single (tuple-ref (var-ref %vec.136) 31))
        (single (tuple-ref (var-ref %vec.136) 30))
        (single (tuple-ref (var-ref %vec.136) 29))
        (single (tuple-ref (var-ref %vec.136) 28))
        (single (tuple-ref (var-ref %vec.136) 27))
        (single (tuple-ref (var-ref %vec.136) 26))
        (single (tuple-ref (var-ref %vec.136) 25))
        (single (tuple-ref (var-ref %vec.136) 24))
        (single (tuple-ref (var-ref %vec.136) 23))
        (single (tuple-ref (var-ref %vec.136) 22))
        (single (tuple-ref (var-ref %vec.136) 21))
        (single (tuple-ref (var-ref %vec.136) 20))
        (single (tuple-ref (var-ref %vec.136) 19))
        (single (tuple-ref (var-ref %vec.136) 18))
        (single (tuple-ref (var-ref %vec.136) 17))
        (single (tuple-ref (var-ref %vec.136) 16))
        (single (tuple-ref (var-ref %vec.136) 15))
        (single (tuple-ref (var-ref %vec.136) 14))
        (single (tuple-ref (var-ref %vec.136) 13))
        (single (tuple-ref (var-ref %vec.136) 12))
        (single (tuple-ref (var-ref %vec.136) 11))
        (single (tuple-ref (var-ref %vec.136) 10))
        (single (tuple-ref (var-ref %vec.136) 9))
        (single (tuple-ref (var-ref %vec.136) 8))
        (single (tuple-ref (var-ref %vec.136) 7))
        (single (tuple-ref (var-ref %vec.136) 6))
        (single (tuple-ref (var-ref %vec.136) 5))
        (single (tuple-ref (var-ref %vec.136) 4))
        (single (tuple-ref (var-ref %vec.136) 3))
        (single (tuple-ref (var-ref %vec.136) 2))
        (single (tuple-ref (var-ref %vec.136) 1))
        (single (tuple-ref (var-ref %vec.136) 0))))))
 (circuit %secp256k1EthereumAddress.102 (exported #f) (pure #t)
   (proof #f) ((%pk.137 (tpoint (curve-secp256k1))))
   (tbytes 20)
   (seq (assert
          (!= (tpoint (curve-secp256k1))
              (var-ref %pk.137)
              (default (tpoint (curve-secp256k1))))
          "Secp256k1Point identity is not a permitted input to secp256k1EthereumAddress")
        (let* (((%hash.138 (tbytes 32)) (call
                                          %keccak256.133
                                          (tuple
                                            (single
                                              (call
                                                %secp256k1BaseBigEndian.134
                                                (call
                                                  %secp256k1PointX.126
                                                  (var-ref %pk.137))))
                                            (single
                                              (call
                                                %secp256k1BaseBigEndian.134
                                                (call
                                                  %secp256k1PointY.127
                                                  (var-ref %pk.137))))))))
          (return
            (bytes-slice (tbytes 32) (var-ref %hash.138) '12 20)))))
 (native %keccak256.133 (entry "__compactRuntime.keccak256" circuit)
   (type-arguments
     (ttuple
       (tvector 32 (tunsigned 255))
       (tvector 32 (tunsigned 255))))
   ((%value.154
      (ttuple
        (tvector 32 (tunsigned 255))
        (tvector 32 (tunsigned 255)))))
   (tbytes 32))
 (native %neg.131
   (entry "__compactRuntime.secp256k1BaseNeg" circuit)
   (type-arguments)
   ((%s.155 (tfield (field-base (curve-secp256k1)))))
   (tfield (field-base (curve-secp256k1))))
 (native %inv.130
   (entry "__compactRuntime.secp256k1BaseInv" circuit)
   (type-arguments)
   ((%s.156 (tfield (field-base (curve-secp256k1)))))
   (tfield (field-base (curve-secp256k1))))
 (native %neg.117
   (entry "__compactRuntime.secp256k1ScalarNeg" circuit)
   (type-arguments)
   ((%s.157 (tfield (field-scalar (curve-secp256k1)))))
   (tfield (field-scalar (curve-secp256k1))))
 (native %inv.114
   (entry "__compactRuntime.secp256k1ScalarInv" circuit)
   (type-arguments)
   ((%s.158 (tfield (field-scalar (curve-secp256k1)))))
   (tfield (field-scalar (curve-secp256k1))))
 (native %secp256k1PointX.126
   (entry "__compactRuntime.secp256k1PointX" circuit)
   (type-arguments) ((%pt.159 (tpoint (curve-secp256k1))))
   (tfield (field-base (curve-secp256k1))))
 (native %secp256k1PointY.127
   (entry "__compactRuntime.secp256k1PointY" circuit)
   (type-arguments) ((%pt.160 (tpoint (curve-secp256k1))))
   (tfield (field-base (curve-secp256k1))))
 (native %ecAdd.120 (entry "__compactRuntime.secp256k1Add" circuit)
   (type-arguments)
   ((%a.161 (tpoint (curve-secp256k1)))
     (%b.162 (tpoint (curve-secp256k1))))
   (tpoint (curve-secp256k1)))
 (native %ecMul.123 (entry "__compactRuntime.secp256k1Mul" circuit)
   (type-arguments)
   ((%a.163 (tpoint (curve-secp256k1)))
     (%b.164 (tfield (field-scalar (curve-secp256k1)))))
   (tpoint (curve-secp256k1)))
 (native %ecMulGenerator.121
   (entry "__compactRuntime.secp256k1MulGenerator" circuit)
   (type-arguments)
   ((%b.165 (tfield (field-scalar (curve-secp256k1)))))
   (tpoint (curve-secp256k1)))
 (witness
   %secret_scalar.113
   ()
   (tfield (field-scalar (curve-secp256k1))))
 (circuit %base_ops.98 (exported #t) (pure #f) (proof #t)
   ((%a.128 (tfield (field-base (curve-secp256k1))))
     (%b.129 (tfield (field-base (curve-secp256k1)))))
   (tvector 5 (tfield (field-base (curve-secp256k1))))
   (let* (((%sum.132 (tfield (field-base (curve-secp256k1)))) (+ (tfield
                                                                   (field-base
                                                                     (curve-secp256k1)))
                                                                 (var-ref
                                                                   %a.128)
                                                                 (var-ref
                                                                   %b.129))))
     (seq (public-ledger %base_cell.101 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %sum.132))))
              (ins (cached #f) (n 1)))
            (var-ref %sum.132))
          (return
            (tuple (single (var-ref %sum.132))
              (single
                (- (tfield (field-base (curve-secp256k1)))
                   (var-ref %a.128)
                   (var-ref %b.129)))
              (single
                (* (tfield (field-base (curve-secp256k1)))
                   (var-ref %a.128)
                   (var-ref %b.129)))
              (single (call %neg.131 (var-ref %a.128)))
              (single (call %inv.130 (var-ref %b.129))))))))
 (circuit %scalar_ops.92 (exported #t) (pure #f) (proof #t)
   ((%a.116 (tfield (field-scalar (curve-secp256k1))))
     (%b.115 (tfield (field-scalar (curve-secp256k1)))))
   (tvector 5 (tfield (field-scalar (curve-secp256k1))))
   (let* (((%k.118 (tfield (field-scalar (curve-secp256k1)))) (call
                                                                %secret_scalar.113)))
     (seq (let* (((%tmp.119
                    (tfield (field-scalar (curve-secp256k1)))) (* (tfield
                                                                    (field-scalar
                                                                      (curve-secp256k1)))
                                                                  (var-ref
                                                                    %a.116)
                                                                  (var-ref
                                                                    %k.118))))
            (public-ledger %scalar_cell.95 write (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %tmp.119))))
                (ins (cached #f) (n 1)))
              (var-ref %tmp.119)))
          (return
            (tuple
              (single
                (+ (tfield (field-scalar (curve-secp256k1)))
                   (var-ref %a.116)
                   (var-ref %b.115)))
              (single
                (- (tfield (field-scalar (curve-secp256k1)))
                   (var-ref %a.116)
                   (var-ref %b.115)))
              (single
                (* (tfield (field-scalar (curve-secp256k1)))
                   (var-ref %a.116)
                   (var-ref %b.115)))
              (single (call %neg.117 (var-ref %a.116)))
              (single (call %inv.114 (var-ref %b.115))))))))
 (circuit %curve_ops.96 (exported #t) (pure #f) (proof #t)
   ((%p.124 (tpoint (curve-secp256k1)))
     (%k.122 (tfield (field-scalar (curve-secp256k1)))))
   (ttuple
     (tfield (field-base (curve-secp256k1)))
     (tfield (field-base (curve-secp256k1)))
     (tboolean)
     (tboolean))
   (let* (((%q.125 (tpoint (curve-secp256k1))) (call
                                                 %ecAdd.120
                                                 (call
                                                   %ecMulGenerator.121
                                                   (var-ref %k.122))
                                                 (call
                                                   %ecMul.123
                                                   (var-ref %p.124)
                                                   (var-ref %k.122)))))
     (seq (public-ledger %point_cell.94 write (2) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 2 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %q.125))))
              (ins (cached #f) (n 1)))
            (var-ref %q.125))
          (return
            (tuple
              (single (call %secp256k1PointX.126 (var-ref %q.125)))
              (single (call %secp256k1PointY.127 (var-ref %q.125)))
              (single
                (== (tpoint (curve-secp256k1))
                    (var-ref %q.125)
                    (var-ref %p.124)))
              (single
                (== (tpoint (curve-secp256k1))
                    (call
                      %ecMulGenerator.121
                      (default (tfield (field-scalar (curve-secp256k1)))))
                    (default (tpoint (curve-secp256k1))))))))))
 (circuit %casts.99 (exported #t) (pure #f) (proof #t)
   ((%b.105 (tbytes 32))
     (%x.106 (tfield (field-base (curve-secp256k1)))))
   (ttuple
     (tfield (field-scalar (curve-secp256k1)))
     (tfield (field-base (curve-secp256k1)))
     (tbytes 32))
   (let* (((%s.107 (tfield (field-scalar (curve-secp256k1)))) (cast-from-bytes
                                                                (tfield
                                                                  (field-scalar
                                                                    (curve-secp256k1)))
                                                                32
                                                                (var-ref
                                                                  %b.105))))
     (seq (public-ledger %scalar_cell.95 write (1) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 1 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %s.107))))
              (ins (cached #f) (n 1)))
            (var-ref %s.107))
          (return
            (tuple
              (single (var-ref %s.107))
              (single
                (cast-from-bytes
                  (tfield (field-base (curve-secp256k1)))
                  32
                  (var-ref %b.105)))
              (single
                (field->bytes
                  32
                  (field-base (curve-secp256k1))
                  (var-ref %x.106))))))))
 (circuit %verify.91 (exported #t) (pure #f) (proof #t)
   ((%msg_hash.110 (tbytes 32))
     (%sig.111
       (tstruct
         Secp256k1EcdsaSignature
         (r (tfield (field-scalar (curve-secp256k1))))
         (s (tfield (field-scalar (curve-secp256k1))))))
     (%pk.109 (tpoint (curve-secp256k1))))
   (tboolean)
   (let* (((%valid.112 (tboolean)) (call
                                     %secp256k1EcdsaVerify.108
                                     (var-ref %msg_hash.110)
                                     (var-ref %sig.111)
                                     (var-ref %pk.109))))
     (seq (public-ledger %verified.93 write (3) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 3 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %valid.112))))
              (ins (cached #f) (n 1)))
            (var-ref %valid.112))
          (return (var-ref %valid.112)))))
 (circuit %eth_address.97 (exported #t) (pure #f) (proof #t)
   ((%pk.103 (tpoint (curve-secp256k1)))) (tbytes 20)
   (let* (((%a.104 (tbytes 20)) (call
                                  %secp256k1EthereumAddress.102
                                  (var-ref %pk.103))))
     (seq (public-ledger %address.100 write (4) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 4 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %a.104))))
              (ins (cached #f) (n 1)))
            (var-ref %a.104))
          (return (var-ref %a.104))))))
