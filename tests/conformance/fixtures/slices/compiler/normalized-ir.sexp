(normalized-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (byte_pair . %byte_pair.47) (index_bytes . %index_bytes.48)
    (slice_bytes_const . %slice_bytes_const.45)
    (slice_bytes_dynamic . %slice_bytes_dynamic.46)
    (slice_then_index . %slice_then_index.43)
    (slice_tuple_const . %slice_tuple_const.44)
    (slice_vector_const . %slice_vector_const.41)
    (slice_vector_dynamic . %slice_vector_dynamic.42)
    (tail_bytes . %tail_bytes.39)
    (tuple_digest . %tuple_digest.40)
    (vector_digest . %vector_digest.38))
  (contract-types)
  (kernel-declaration (%kernel.74 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%byte_pair.47
        (0)
        (exported #t)
        (__compact_Cell (tunsigned 65535)))
      (%vector_digest.38
        (1)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%tuple_digest.40
        (2)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%tail_bytes.39
        (3)
        (exported #t)
        (__compact_Cell (tbytes 4))))
    (constructor () (tuple)))
  (circuit %pack3.59 (exported #f) (pure #t) (proof #f)
    ((%v.71 (tvector 3 (tfield (field-native)))))
    (tfield (field-native))
    (return
      (+ (tfield (field-native))
         (+ (tfield (field-native))
            (* (tfield (field-native))
               (tuple-ref (var-ref %v.71) 0)
               (safe-cast
                 (tfield (field-native))
                 (tunsigned 1000000)
                 '1000000))
            (* (tfield (field-native))
               (tuple-ref (var-ref %v.71) 1)
               (safe-cast (tfield (field-native)) (tunsigned 1000) '1000)))
         (tuple-ref (var-ref %v.71) 2))))
  (circuit %index_bytes.48 (exported #t) (pure #f) (proof #t)
    ((%b.72 (tbytes 8))) (tunsigned 65535)
    (let* (((%packed.73 (tunsigned 65535)) (+ (tunsigned 65535)
                                              (safe-cast
                                                (tunsigned 65535)
                                                (tunsigned 65280)
                                                (* (tunsigned 65280)
                                                   (safe-cast
                                                     (tunsigned 65280)
                                                     (tunsigned 255)
                                                     (bytes-ref
                                                       (tbytes 8)
                                                       (var-ref %b.72)
                                                       '2))
                                                   (safe-cast
                                                     (tunsigned 65280)
                                                     (tunsigned 256)
                                                     '256)))
                                              (safe-cast
                                                (tunsigned 65535)
                                                (tunsigned 255)
                                                (bytes-ref
                                                  (tbytes 8)
                                                  (var-ref %b.72)
                                                  '5)))))
      (seq (public-ledger %byte_pair.47 (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %packed.73))))
               (ins (cached #f) (n 1)))
             (var-ref %packed.73))
           (return (var-ref %packed.73)))))
  (circuit %slice_bytes_const.45 (exported #t) (pure #f) (proof #t)
    ((%b.66 (tbytes 8))) (tbytes 4)
    (let* (((%tail.67 (tbytes 4)) (bytes-slice
                                    (tbytes 8)
                                    (var-ref %b.66)
                                    '3
                                    4)))
      (seq (public-ledger %tail_bytes.39 (3) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 3 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tail.67))))
               (ins (cached #f) (n 1)))
             (var-ref %tail.67))
           (return (var-ref %tail.67)))))
  (circuit %slice_bytes_dynamic.46 (exported #t) (pure #f) (proof #t)
    ((%b.68 (tbytes 8))) (tbytes 4)
    (let* (((%start.69 (tunsigned 1)) '1))
      (let* (((%tail.70 (tbytes 4)) (bytes-slice
                                      (tbytes 8)
                                      (var-ref %b.68)
                                      (var-ref %start.69)
                                      4)))
        (seq (public-ledger %tail_bytes.39 (3) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 3 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %tail.70))))
                 (ins (cached #f) (n 1)))
               (var-ref %tail.70))
             (return (var-ref %tail.70))))))
  (circuit %slice_then_index.43 (exported #t) (pure #f) (proof #t)
    ((%b.60 (tbytes 8))) (tunsigned 65535)
    (let* (((%tail.61 (tbytes 4)) (bytes-slice
                                    (tbytes 8)
                                    (var-ref %b.60)
                                    '3
                                    4)))
      (let* (((%packed.62 (tunsigned 65535)) (+ (tunsigned 65535)
                                                (safe-cast
                                                  (tunsigned 65535)
                                                  (tunsigned 65280)
                                                  (* (tunsigned 65280)
                                                     (safe-cast
                                                       (tunsigned 65280)
                                                       (tunsigned 255)
                                                       (bytes-ref
                                                         (tbytes 4)
                                                         (var-ref %tail.61)
                                                         '0))
                                                     (safe-cast
                                                       (tunsigned 65280)
                                                       (tunsigned 256)
                                                       '256)))
                                                (safe-cast
                                                  (tunsigned 65535)
                                                  (tunsigned 255)
                                                  (bytes-ref
                                                    (tbytes 4)
                                                    (var-ref %tail.61)
                                                    '3)))))
        (seq (public-ledger %byte_pair.47 (0) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 0 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %packed.62))))
                 (ins (cached #f) (n 1)))
               (var-ref %packed.62))
             (return (var-ref %packed.62))))))
  (circuit %slice_vector_const.41 (exported #t) (pure #f) (proof #t)
    ((%xs.63 (tvector 6 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%mid.64 (tvector 3 (tfield (field-native)))) (tuple-slice
                                                            (tvector
                                                              6
                                                              (tfield
                                                                (field-native)))
                                                            (var-ref
                                                              %xs.63)
                                                            2
                                                            3)))
      (let* (((%packed.65 (tfield (field-native))) (call
                                                     %pack3.59
                                                     (var-ref %mid.64))))
        (seq (public-ledger %vector_digest.38 (1) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 1 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %packed.65))))
                 (ins (cached #f) (n 1)))
               (var-ref %packed.65))
             (return (var-ref %packed.65))))))
  (circuit %slice_tuple_const.44 (exported #t) (pure #f) (proof #t)
    ((%a.50 (tunsigned 255))
      (%b.51 (tunsigned 65535))
      (%c.49 (tfield (field-native))))
    (tfield (field-native))
    (let* (((%row.52
              (ttuple
                (tunsigned 255)
                (tunsigned 65535)
                (tfield (field-native))
                (tfield (field-native)))) (tuple
                                            (single (var-ref %a.50))
                                            (single (var-ref %b.51))
                                            (single (var-ref %c.49))
                                            (single
                                              (safe-cast
                                                (tfield (field-native))
                                                (tunsigned 7)
                                                '7)))))
      (let* (((%mid.53
                (ttuple (tunsigned 65535) (tfield (field-native)))) (tuple-slice
                                                                      (ttuple
                                                                        (tunsigned
                                                                          255)
                                                                        (tunsigned
                                                                          65535)
                                                                        (tfield
                                                                          (field-native))
                                                                        (tfield
                                                                          (field-native)))
                                                                      (var-ref
                                                                        %row.52)
                                                                      1
                                                                      2)))
        (let* (((%packed.54 (tfield (field-native))) (+ (tfield
                                                          (field-native))
                                                        (* (tfield
                                                             (field-native))
                                                           (safe-cast
                                                             (tfield
                                                               (field-native))
                                                             (tunsigned
                                                               65535)
                                                             (tuple-ref
                                                               (var-ref
                                                                 %mid.53)
                                                               0))
                                                           (safe-cast
                                                             (tfield
                                                               (field-native))
                                                             (tunsigned
                                                               1000)
                                                             '1000))
                                                        (tuple-ref
                                                          (var-ref %mid.53)
                                                          1))))
          (seq (public-ledger %tuple_digest.40 (2) write (ttuple)
                 (instructions
                   (push
                     (storage #f)
                     (value (state-value cell (align 2 1))))
                   (push
                     (storage #t)
                     (value (state-value cell (var-ref %packed.54))))
                   (ins (cached #f) (n 1)))
                 (var-ref %packed.54))
               (return (var-ref %packed.54)))))))
  (circuit %slice_vector_dynamic.42 (exported #t) (pure #f) (proof #t)
    ((%xs.55 (tvector 6 (tfield (field-native)))))
    (tfield (field-native))
    (let* (((%start.56 (tunsigned 1)) '1))
      (let* (((%mid.57 (tvector 3 (tfield (field-native)))) (vector-slice
                                                              (tvector
                                                                6
                                                                (tfield
                                                                  (field-native)))
                                                              (var-ref
                                                                %xs.55)
                                                              (var-ref
                                                                %start.56)
                                                              3)))
        (let* (((%packed.58 (tfield (field-native))) (call
                                                       %pack3.59
                                                       (var-ref %mid.57))))
          (seq (public-ledger %vector_digest.38 (1) write (ttuple)
                 (instructions
                   (push
                     (storage #f)
                     (value (state-value cell (align 1 1))))
                   (push
                     (storage #t)
                     (value (state-value cell (var-ref %packed.58))))
                   (ins (cached #f) (n 1)))
                 (var-ref %packed.58))
               (return (var-ref %packed.58))))))))
