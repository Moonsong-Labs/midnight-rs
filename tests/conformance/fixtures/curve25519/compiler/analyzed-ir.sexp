(analyzed-ir (compiler-version "0.35.103-dev")
 (language-version "0.27.0") (runtime-version "0.20.101")
 (exports (base_cell . %base_cell.216) (base_ops . %base_ops.217)
   (casts . %casts.214) (curve_ops . %curve_ops.215)
   (point_cell . %point_cell.212)
   (scalar_cell . %scalar_cell.213)
   (scalar_ops . %scalar_ops.210) (verified . %verified.211)
   (verify . %verify.209))
 (contract-types)
 (kernel-declaration (%kernel.268 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%base_cell.216
       (0)
       (exported #t)
       (__compact_Cell (tfield (field-base (curve-curve25519)))))
     (%scalar_cell.213
       (1)
       (exported #t)
       (__compact_Cell (tfield (field-scalar (curve-curve25519)))))
     (%point_cell.212
       (2)
       (exported #t)
       (__compact_Cell (tpoint (curve-curve25519))))
     (%verified.211
       (3)
       (exported #t)
       (__compact_Cell (tboolean))))
   (constructor () (tuple)))
 (circuit %curve25519BaseIsOdd.256 (exported #f) (pure #t) (proof #f)
   ((%x.257 (tfield (field-base (curve-curve25519)))))
   (tboolean)
   (let* (((%b.258 (tunsigned 255)) (tuple-ref
                                      (bytes->vector
                                        32
                                        (field->bytes
                                          32
                                          (field-base (curve-curve25519))
                                          (var-ref %x.257)))
                                      0)))
     (let* (((%b6.259 (tunsigned 255)) (if (>= 8
                                               (var-ref %b.258)
                                               (safe-cast
                                                 (tunsigned 255)
                                                 (tunsigned 128)
                                                 '128))
                                           (seq (assert
                                                  (>= 8
                                                      (var-ref %b.258)
                                                      (safe-cast
                                                        (tunsigned 255)
                                                        (tunsigned 128)
                                                        '128))
                                                  "result of subtraction would be negative")
                                                (- (tunsigned 255)
                                                   (var-ref %b.258)
                                                   (safe-cast
                                                     (tunsigned 255)
                                                     (tunsigned 128)
                                                     '128)))
                                           (var-ref %b.258))))
       (let* (((%b5.260 (tunsigned 255)) (if (>= 8
                                                 (var-ref %b6.259)
                                                 (safe-cast
                                                   (tunsigned 255)
                                                   (tunsigned 64)
                                                   '64))
                                             (seq (assert
                                                    (>= 8
                                                        (var-ref %b6.259)
                                                        (safe-cast
                                                          (tunsigned 255)
                                                          (tunsigned 64)
                                                          '64))
                                                    "result of subtraction would be negative")
                                                  (- (tunsigned 255)
                                                     (var-ref %b6.259)
                                                     (safe-cast
                                                       (tunsigned 255)
                                                       (tunsigned 64)
                                                       '64)))
                                             (var-ref %b6.259))))
         (let* (((%b4.261 (tunsigned 255)) (if (>= 8
                                                   (var-ref %b5.260)
                                                   (safe-cast
                                                     (tunsigned 255)
                                                     (tunsigned 32)
                                                     '32))
                                               (seq (assert
                                                      (>= 8
                                                          (var-ref %b5.260)
                                                          (safe-cast
                                                            (tunsigned 255)
                                                            (tunsigned 32)
                                                            '32))
                                                      "result of subtraction would be negative")
                                                    (- (tunsigned 255)
                                                       (var-ref %b5.260)
                                                       (safe-cast
                                                         (tunsigned 255)
                                                         (tunsigned 32)
                                                         '32)))
                                               (var-ref %b5.260))))
           (let* (((%b3.262 (tunsigned 255)) (if (>= 8
                                                     (var-ref %b4.261)
                                                     (safe-cast
                                                       (tunsigned 255)
                                                       (tunsigned 16)
                                                       '16))
                                                 (seq (assert
                                                        (>= 8
                                                            (var-ref
                                                              %b4.261)
                                                            (safe-cast
                                                              (tunsigned
                                                                255)
                                                              (tunsigned
                                                                16)
                                                              '16))
                                                        "result of subtraction would be negative")
                                                      (- (tunsigned 255)
                                                         (var-ref %b4.261)
                                                         (safe-cast
                                                           (tunsigned 255)
                                                           (tunsigned 16)
                                                           '16)))
                                                 (var-ref %b4.261))))
             (let* (((%b2.263 (tunsigned 255)) (if (>= 8
                                                       (var-ref %b3.262)
                                                       (safe-cast
                                                         (tunsigned 255)
                                                         (tunsigned 8)
                                                         '8))
                                                   (seq (assert
                                                          (>= 8
                                                              (var-ref
                                                                %b3.262)
                                                              (safe-cast
                                                                (tunsigned
                                                                  255)
                                                                (tunsigned
                                                                  8)
                                                                '8))
                                                          "result of subtraction would be negative")
                                                        (- (tunsigned 255)
                                                           (var-ref
                                                             %b3.262)
                                                           (safe-cast
                                                             (tunsigned
                                                               255)
                                                             (tunsigned 8)
                                                             '8)))
                                                   (var-ref %b3.262))))
               (let* (((%b1.264 (tunsigned 255)) (if (>= 8
                                                         (var-ref %b2.263)
                                                         (safe-cast
                                                           (tunsigned 255)
                                                           (tunsigned 4)
                                                           '4))
                                                     (seq (assert
                                                            (>= 8
                                                                (var-ref
                                                                  %b2.263)
                                                                (safe-cast
                                                                  (tunsigned
                                                                    255)
                                                                  (tunsigned
                                                                    4)
                                                                  '4))
                                                            "result of subtraction would be negative")
                                                          (- (tunsigned
                                                               255)
                                                             (var-ref
                                                               %b2.263)
                                                             (safe-cast
                                                               (tunsigned
                                                                 255)
                                                               (tunsigned
                                                                 4)
                                                               '4)))
                                                     (var-ref %b2.263))))
                 (return
                   (== (tunsigned 255)
                       (if (>= 8
                               (var-ref %b1.264)
                               (safe-cast
                                 (tunsigned 255)
                                 (tunsigned 2)
                                 '2))
                           (seq (assert
                                  (>= 8
                                      (var-ref %b1.264)
                                      (safe-cast
                                        (tunsigned 255)
                                        (tunsigned 2)
                                        '2))
                                  "result of subtraction would be negative")
                                (- (tunsigned 255)
                                   (var-ref %b1.264)
                                   (safe-cast
                                     (tunsigned 255)
                                     (tunsigned 2)
                                     '2)))
                           (var-ref %b1.264))
                       (safe-cast
                         (tunsigned 255)
                         (tunsigned 1)
                         '1)))))))))))
 (circuit %ed25519Encode.251 (exported #f) (pure #t) (proof #f)
   ((%p.265 (tpoint (curve-curve25519)))) (tbytes 32)
   (let* (((%y.266 (tvector 32 (tunsigned 255))) (bytes->vector
                                                   32
                                                   (field->bytes
                                                     32
                                                     (field-base
                                                       (curve-curve25519))
                                                     (call
                                                       %curve25519PointY.230
                                                       (var-ref
                                                         %p.265))))))
     (let* (((%sign.267 (tunsigned 128)) (if (call
                                               %curve25519BaseIsOdd.256
                                               (call
                                                 %curve25519PointX.229
                                                 (var-ref %p.265)))
                                             '128
                                             (safe-cast
                                               (tunsigned 128)
                                               (tunsigned 0)
                                               '0))))
       (return
         (vector->bytes
           32
           (vector (single (tuple-ref (var-ref %y.266) 0))
            (single (tuple-ref (var-ref %y.266) 1))
            (single (tuple-ref (var-ref %y.266) 2))
            (single (tuple-ref (var-ref %y.266) 3))
            (single (tuple-ref (var-ref %y.266) 4))
            (single (tuple-ref (var-ref %y.266) 5))
            (single (tuple-ref (var-ref %y.266) 6))
            (single (tuple-ref (var-ref %y.266) 7))
            (single (tuple-ref (var-ref %y.266) 8))
            (single (tuple-ref (var-ref %y.266) 9))
            (single (tuple-ref (var-ref %y.266) 10))
            (single (tuple-ref (var-ref %y.266) 11))
            (single (tuple-ref (var-ref %y.266) 12))
            (single (tuple-ref (var-ref %y.266) 13))
            (single (tuple-ref (var-ref %y.266) 14))
            (single (tuple-ref (var-ref %y.266) 15))
            (single (tuple-ref (var-ref %y.266) 16))
            (single (tuple-ref (var-ref %y.266) 17))
            (single (tuple-ref (var-ref %y.266) 18))
            (single (tuple-ref (var-ref %y.266) 19))
            (single (tuple-ref (var-ref %y.266) 20))
            (single (tuple-ref (var-ref %y.266) 21))
            (single (tuple-ref (var-ref %y.266) 22))
            (single (tuple-ref (var-ref %y.266) 23))
            (single (tuple-ref (var-ref %y.266) 24))
            (single (tuple-ref (var-ref %y.266) 25))
            (single (tuple-ref (var-ref %y.266) 26))
            (single (tuple-ref (var-ref %y.266) 27))
            (single (tuple-ref (var-ref %y.266) 28))
            (single (tuple-ref (var-ref %y.266) 29))
            (single (tuple-ref (var-ref %y.266) 30))
            (single
              (downcast-unsigned
                383
                255
                (+ (tunsigned 383)
                   (safe-cast
                     (tunsigned 383)
                     (tunsigned 255)
                     (tuple-ref (var-ref %y.266) 31))
                   (safe-cast
                     (tunsigned 383)
                     (tunsigned 128)
                     (var-ref %sign.267)))))))))))
 (circuit %ed25519Verify.218 (exported #f) (pure #t) (proof #f)
   ((%msg.250 (tbytes 1))
     (%sig.247
       (tstruct
         Ed25519Signature
         (r (tpoint (curve-curve25519)))
         (s (tfield (field-scalar (curve-curve25519))))))
     (%pk.253 (tpoint (curve-curve25519))))
   (tboolean)
   (seq (assert
          (!= (tbytes 32)
              (field->bytes
                32
                (field-base (curve-curve25519))
                (call %curve25519PointX.229 (var-ref %pk.253)))
              '#vu8(0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
                    0 0 0 0 0))
          "Curve25519Point identity is not a permitted ed25519Verify verification key")
        (let* (((%__compact_pattern_tmp1.248
                  (tstruct
                    Ed25519Signature
                    (r (tpoint (curve-curve25519)))
                    (s (tfield (field-scalar (curve-curve25519)))))) (var-ref
                                                                       %sig.247)))
          (let* (((%r.252 (tpoint (curve-curve25519))) (elt-ref
                                                         (var-ref
                                                           %__compact_pattern_tmp1.248)
                                                         r
                                                         0)))
            (let* (((%s.254 (tfield (field-scalar (curve-curve25519)))) (elt-ref
                                                                          (var-ref
                                                                            %__compact_pattern_tmp1.248)
                                                                          s
                                                                          1)))
              (let* (((%k.255 (tfield (field-scalar (curve-curve25519)))) (cast-from-bytes
                                                                            (tfield
                                                                              (field-scalar
                                                                                (curve-curve25519)))
                                                                            64
                                                                            (call
                                                                              %sha512.249
                                                                              (tuple
                                                                                (single
                                                                                  (call
                                                                                    %ed25519Encode.251
                                                                                    (var-ref
                                                                                      %r.252)))
                                                                                (single
                                                                                  (call
                                                                                    %ed25519Encode.251
                                                                                    (var-ref
                                                                                      %pk.253)))
                                                                                (single
                                                                                  (var-ref
                                                                                    %msg.250)))))))
                (return
                  (== (tpoint (curve-curve25519))
                      (call %ecMulGenerator.224 (var-ref %s.254))
                      (call
                        %ecAdd.223
                        (var-ref %r.252)
                        (call
                          %ecMul.226
                          (var-ref %pk.253)
                          (var-ref %k.255)))))))))))
 (native %sha512.249 (entry "__compactRuntime.sha512" circuit)
   (type-arguments (ttuple (tbytes 32) (tbytes 32) (tbytes 1)))
   ((%value.269 (ttuple (tbytes 32) (tbytes 32) (tbytes 1))))
   (tbytes 64))
 (native %neg.238
   (entry "__compactRuntime.curve25519BaseNeg" circuit)
   (type-arguments)
   ((%s.270 (tfield (field-base (curve-curve25519)))))
   (tfield (field-base (curve-curve25519))))
 (native %inv.237
   (entry "__compactRuntime.curve25519BaseInv" circuit)
   (type-arguments)
   ((%s.271 (tfield (field-base (curve-curve25519)))))
   (tfield (field-base (curve-curve25519))))
 (native %neg.244
   (entry "__compactRuntime.curve25519ScalarNeg" circuit)
   (type-arguments)
   ((%s.272 (tfield (field-scalar (curve-curve25519)))))
   (tfield (field-scalar (curve-curve25519))))
 (native %inv.241
   (entry "__compactRuntime.curve25519ScalarInv" circuit)
   (type-arguments)
   ((%s.273 (tfield (field-scalar (curve-curve25519)))))
   (tfield (field-scalar (curve-curve25519))))
 (native %curve25519PointX.229
   (entry "__compactRuntime.curve25519PointX" circuit)
   (type-arguments) ((%pt.274 (tpoint (curve-curve25519))))
   (tfield (field-base (curve-curve25519))))
 (native %curve25519PointY.230
   (entry "__compactRuntime.curve25519PointY" circuit)
   (type-arguments) ((%pt.275 (tpoint (curve-curve25519))))
   (tfield (field-base (curve-curve25519))))
 (native %ecAdd.223 (entry "__compactRuntime.curve25519Add" circuit)
   (type-arguments)
   ((%a.276 (tpoint (curve-curve25519)))
     (%b.277 (tpoint (curve-curve25519))))
   (tpoint (curve-curve25519)))
 (native %ecMul.226 (entry "__compactRuntime.curve25519Mul" circuit)
   (type-arguments)
   ((%a.278 (tpoint (curve-curve25519)))
     (%b.279 (tfield (field-scalar (curve-curve25519)))))
   (tpoint (curve-curve25519)))
 (native %ecMulGenerator.224
   (entry "__compactRuntime.curve25519MulGenerator" circuit)
   (type-arguments)
   ((%b.280 (tfield (field-scalar (curve-curve25519)))))
   (tpoint (curve-curve25519)))
 (witness
   %secret_scalar.240
   ()
   (tfield (field-scalar (curve-curve25519))))
 (circuit %base_ops.217 (exported #t) (pure #f) (proof #t)
   ((%a.235 (tfield (field-base (curve-curve25519))))
     (%b.236 (tfield (field-base (curve-curve25519)))))
   (tvector 5 (tfield (field-base (curve-curve25519))))
   (let* (((%sum.239 (tfield (field-base (curve-curve25519)))) (+ (tfield
                                                                    (field-base
                                                                      (curve-curve25519)))
                                                                  (var-ref
                                                                    %a.235)
                                                                  (var-ref
                                                                    %b.236))))
     (seq (public-ledger %base_cell.216 write (0) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 0 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %sum.239))))
              (ins (cached #f) (n 1)))
            (var-ref %sum.239))
          (return
            (tuple (single (var-ref %sum.239))
              (single
                (- (tfield (field-base (curve-curve25519)))
                   (var-ref %a.235)
                   (var-ref %b.236)))
              (single
                (* (tfield (field-base (curve-curve25519)))
                   (var-ref %a.235)
                   (var-ref %b.236)))
              (single (call %neg.238 (var-ref %a.235)))
              (single (call %inv.237 (var-ref %b.236))))))))
 (circuit %scalar_ops.210 (exported #t) (pure #f) (proof #t)
   ((%a.243 (tfield (field-scalar (curve-curve25519))))
     (%b.242 (tfield (field-scalar (curve-curve25519)))))
   (tvector 5 (tfield (field-scalar (curve-curve25519))))
   (let* (((%k.245 (tfield (field-scalar (curve-curve25519)))) (call
                                                                 %secret_scalar.240)))
     (seq (let* (((%tmp.246
                    (tfield (field-scalar (curve-curve25519)))) (* (tfield
                                                                     (field-scalar
                                                                       (curve-curve25519)))
                                                                   (var-ref
                                                                     %a.243)
                                                                   (var-ref
                                                                     %k.245))))
            (public-ledger %scalar_cell.213 write (1) write (ttuple)
              (instructions
                (push (storage #f) (value (state-value cell (align 1 1))))
                (push
                  (storage #t)
                  (value (state-value cell (var-ref %tmp.246))))
                (ins (cached #f) (n 1)))
              (var-ref %tmp.246)))
          (return
            (tuple
              (single
                (+ (tfield (field-scalar (curve-curve25519)))
                   (var-ref %a.243)
                   (var-ref %b.242)))
              (single
                (- (tfield (field-scalar (curve-curve25519)))
                   (var-ref %a.243)
                   (var-ref %b.242)))
              (single
                (* (tfield (field-scalar (curve-curve25519)))
                   (var-ref %a.243)
                   (var-ref %b.242)))
              (single (call %neg.244 (var-ref %a.243)))
              (single (call %inv.241 (var-ref %b.242))))))))
 (circuit %curve_ops.215 (exported #t) (pure #f) (proof #t)
   ((%p.227 (tpoint (curve-curve25519)))
     (%k.225 (tfield (field-scalar (curve-curve25519)))))
   (ttuple
     (tfield (field-base (curve-curve25519)))
     (tfield (field-base (curve-curve25519)))
     (tboolean))
   (let* (((%q.228 (tpoint (curve-curve25519))) (call
                                                  %ecAdd.223
                                                  (call
                                                    %ecMulGenerator.224
                                                    (var-ref %k.225))
                                                  (call
                                                    %ecMul.226
                                                    (var-ref %p.227)
                                                    (var-ref %k.225)))))
     (seq (public-ledger %point_cell.212 write (2) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 2 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %q.228))))
              (ins (cached #f) (n 1)))
            (var-ref %q.228))
          (return
            (tuple
              (single (call %curve25519PointX.229 (var-ref %q.228)))
              (single (call %curve25519PointY.230 (var-ref %q.228)))
              (single
                (== (tpoint (curve-curve25519))
                    (var-ref %q.228)
                    (var-ref %p.227))))))))
 (circuit %casts.214 (exported #t) (pure #f) (proof #t)
   ((%b.231 (tbytes 32))
     (%x.232 (tfield (field-base (curve-curve25519))))
     (%w.233 (tbytes 64)))
   (ttuple
     (tfield (field-scalar (curve-curve25519)))
     (tfield (field-base (curve-curve25519)))
     (tbytes 32)
     (tfield (field-scalar (curve-curve25519))))
   (let* (((%s.234 (tfield (field-scalar (curve-curve25519)))) (cast-from-bytes
                                                                 (tfield
                                                                   (field-scalar
                                                                     (curve-curve25519)))
                                                                 32
                                                                 (var-ref
                                                                   %b.231))))
     (seq (public-ledger %scalar_cell.213 write (1) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 1 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %s.234))))
              (ins (cached #f) (n 1)))
            (var-ref %s.234))
          (return
            (tuple
              (single (var-ref %s.234))
              (single
                (cast-from-bytes
                  (tfield (field-base (curve-curve25519)))
                  32
                  (var-ref %b.231)))
              (single
                (field->bytes
                  32
                  (field-base (curve-curve25519))
                  (var-ref %x.232)))
              (single
                (cast-from-bytes
                  (tfield (field-scalar (curve-curve25519)))
                  64
                  (var-ref %w.233))))))))
 (circuit %verify.209 (exported #t) (pure #f) (proof #t)
   ((%msg.220 (tbytes 1))
     (%sig.221
       (tstruct
         Ed25519Signature
         (r (tpoint (curve-curve25519)))
         (s (tfield (field-scalar (curve-curve25519))))))
     (%pk.219 (tpoint (curve-curve25519))))
   (tboolean)
   (let* (((%valid.222 (tboolean)) (call
                                     %ed25519Verify.218
                                     (var-ref %msg.220)
                                     (var-ref %sig.221)
                                     (var-ref %pk.219))))
     (seq (public-ledger %verified.211 write (3) write (ttuple)
            (instructions
              (push (storage #f) (value (state-value cell (align 3 1))))
              (push
                (storage #t)
                (value (state-value cell (var-ref %valid.222))))
              (ins (cached #f) (n 1)))
            (var-ref %valid.222))
          (return (var-ref %valid.222))))))
