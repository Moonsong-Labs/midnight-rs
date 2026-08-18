(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (claim_deposit . %claim_deposit.128)
   (egress_jobs . %egress_jobs.129)
   (fee_token . %fee_token.126)
   (fulfill_signing_request . %fulfill_signing_request.127)
   (next_job_id . %next_job_id.124)
   (next_signing_request_id . %next_signing_request_id.125)
   (processed_attestations . %processed_attestations.122)
   (sign . %sign.123) (signing_fee . %signing_fee.120)
   (signing_requests . %signing_requests.121)
   (threshold . %threshold.118)
   (unclaimed_deposits . %unclaimed_deposits.119)
   (validators . %validators.116) (withdraw . %withdraw.117)
   (witness_deposit . %witness_deposit.114)
   (witness_egress . %witness_egress.115))
 (contract-types)
 (kernel-declaration (%kernel.176 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%threshold.118
       (0)
       (exported #t)
       (__compact_Cell (tunsigned 255)))
     (%validators.116
       (1)
       (exported #t)
       (Set (tpoint (curve-jubjub))))
     (%unclaimed_deposits.119
       (2)
       (exported #t)
       (Map (tbytes 32)
            (tstruct
              UnclaimedDeposit
              (amount (tunsigned 340282366920938463463374607431768211455))
              (token_ref (tbytes 32)))))
     (%next_job_id.124 (3) (exported #t) (Counter))
     (%egress_jobs.129
       (4)
       (exported #t)
       (Map (tfield (field-native))
            (tstruct EgressJob
              (id (tunsigned 340282366920938463463374607431768211455))
              (destination (tbytes 32)) (token_ref (tbytes 32))
              (amount (tunsigned 340282366920938463463374607431768211455))
              (status (tenum JobStatus pending completed)))))
     (%processed_attestations.122
       (5)
       (exported #t)
       (Set (tbytes 32)))
     (%signing_fee.120
       (6)
       (exported #t)
       (__compact_Cell (tunsigned 18446744073709551615)))
     (%fee_token.126
       (7)
       (exported #t)
       (__compact_Cell (tbytes 32)))
     (%next_signing_request_id.125 (8) (exported #t) (Counter))
     (%signing_requests.121
       (9)
       (exported #t)
       (Map (tfield (field-native))
            (tstruct SigningRequest (entity_id (tbytes 32))
              (domain_id (tunsigned 255)) (payload (tbytes 32))
              (status (tenum SigningRequestStatus pending fulfilled))
              (signature (tbytes 64))))))
   (constructor () (tuple)))
 (export-typedef
   JobStatus
   ()
   (tenum JobStatus pending completed))
 (export-typedef
   SigningRequestStatus
   ()
   (tenum SigningRequestStatus pending fulfilled))
 (export-typedef
   EgressJob
   ()
   (tstruct EgressJob
     (id (tunsigned 340282366920938463463374607431768211455))
     (destination (tbytes 32)) (token_ref (tbytes 32))
     (amount (tunsigned 340282366920938463463374607431768211455))
     (status (tenum JobStatus pending completed))))
 (export-typedef
   UnclaimedDeposit
   ()
   (tstruct
     UnclaimedDeposit
     (amount (tunsigned 340282366920938463463374607431768211455))
     (token_ref (tbytes 32))))
 (export-typedef
   SigningRequest
   ()
   (tstruct SigningRequest (entity_id (tbytes 32))
     (domain_id (tunsigned 255)) (payload (tbytes 32))
     (status (tenum SigningRequestStatus pending fulfilled))
     (signature (tbytes 64))))
 (export-typedef
   ValidatorSignature
   ()
   (tstruct
     ValidatorSignature
     (pk (tpoint (curve-jubjub)))
     (r (tpoint (curve-jubjub)))
     (s (tfield (field-native)))))
 (circuit %right.174 (exported #f) (pure #t) (proof #f)
   ((%value.175 (tstruct ContractAddress (bytes (tbytes 32)))))
   (tstruct
     Either
     (is_left (tboolean))
     (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
     (right (tstruct ContractAddress (bytes (tbytes 32)))))
   (return
     (new (tstruct
            Either
            (is_left (tboolean))
            (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
            (right (tstruct ContractAddress (bytes (tbytes 32)))))
          '#f
          (default (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
          (var-ref %value.175))))
 (circuit %receiveShielded.145 (exported #f) (pure #f) (proof #f)
   ((%coin.177
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455)))))
   (ttuple)
   (seq (let* (((%recipient.178
                  (tstruct
                    Either
                    (is_left (tboolean))
                    (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
                    (right (tstruct ContractAddress (bytes (tbytes 32)))))) (call
                                                                              %right.174
                                                                              (public-ledger
                                                                                %kernel.176
                                                                                read
                                                                                ()
                                                                                self
                                                                                (tstruct
                                                                                  ContractAddress
                                                                                  (bytes
                                                                                    (tbytes
                                                                                      32)))
                                                                                (instructions
                                                                                  (dup (n 2))
                                                                                  (idx (cached
                                                                                         #t)
                                                                                       (pushPath
                                                                                         #f)
                                                                                       (path
                                                                                         ((align
                                                                                            0
                                                                                            1))))
                                                                                  (popeq
                                                                                    (cached
                                                                                      #t)
                                                                                    (result
                                                                                      (void))))))))
          (seq (call
                 %createZswapOutput.169
                 (var-ref %coin.177)
                 (var-ref %recipient.178))
               (let* (((%tmp.179 (tbytes 32)) (call
                                                %coinCommitment.170
                                                (var-ref %coin.177)
                                                (var-ref %recipient.178))))
                 (public-ledger %kernel.176 update () claimZswapCoinReceive (ttuple)
                   (instructions (swap (n 0))
                     (idx (cached #t) (pushPath #t) (path ((align 1 1))))
                     (push
                       (storage #f)
                       (value (state-value cell (var-ref %tmp.179))))
                     (push (storage #f) (value (state-value null)))
                     (ins (cached #t) (n 2)) (swap (n 0)))
                   (var-ref %tmp.179)))))
        (return (tuple))))
 (circuit %coinCommitment.170 (exported #f) (pure #t) (proof #f)
   ((%coin.173
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%recipient.172
       (tstruct
         Either
         (is_left (tboolean))
         (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
         (right (tstruct ContractAddress (bytes (tbytes 32)))))))
   (tbytes 32)
   (return
     (call
       %persistentHash.171
       (new (tstruct CoinPreimage (domain_sep (tbytes 21))
              (info
                (tstruct
                  ShieldedCoinInfo
                  (nonce (tbytes 32))
                  (color (tbytes 32))
                  (value
                    (tunsigned 340282366920938463463374607431768211455))))
              (dataType (tboolean)) (data (tbytes 32)))
            '#vu8(109 105 100 110 105 103 104 116 58 122 115 119 97 112
                  45 99 99 91 118 49 93)
            (var-ref %coin.173)
            (elt-ref (var-ref %recipient.172) is_left 0)
            (if (elt-ref (var-ref %recipient.172) is_left 0)
                (elt-ref (elt-ref (var-ref %recipient.172) left 1) bytes 0)
                (elt-ref
                  (elt-ref (var-ref %recipient.172) right 2)
                  bytes
                  0))))))
 (native %persistentHash.171
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments
     (tstruct CoinPreimage (domain_sep (tbytes 21))
       (info
         (tstruct
           ShieldedCoinInfo
           (nonce (tbytes 32))
           (color (tbytes 32))
           (value
             (tunsigned 340282366920938463463374607431768211455))))
       (dataType (tboolean)) (data (tbytes 32))))
   ((%value.180
      (tstruct CoinPreimage (domain_sep (tbytes 21))
        (info
          (tstruct
            ShieldedCoinInfo
            (nonce (tbytes 32))
            (color (tbytes 32))
            (value
              (tunsigned 340282366920938463463374607431768211455))))
        (dataType (tboolean)) (data (tbytes 32)))))
   (tbytes 32))
 (native %persistentHash.132
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tbytes 64)) ((%value.181 (tbytes 64)))
   (tbytes 32))
 (native %createZswapOutput.169
   (entry "__compactRuntime.createZswapOutput" witness)
   (type-arguments)
   ((%coin.182
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%recipient.183
       (tstruct
         Either
         (is_left (tboolean))
         (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
         (right (tstruct ContractAddress (bytes (tbytes 32)))))))
   (ttuple))
 (circuit %count_valid_sig.137 (exported #f) (pure #t) (proof #f)
   ((%sigs.166
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native)))))))))
   (tunsigned 255)
   (return
     (fold
       9
       (circuit
         ((%a.165 (tunsigned 255))
           (%s.164
             (tstruct
               Maybe
               (is_some (tboolean))
               (value
                 (tstruct
                   ValidatorSignature
                   (pk (tpoint (curve-jubjub)))
                   (r (tpoint (curve-jubjub)))
                   (s (tfield (field-native))))))))
         (tunsigned 255)
         (return
           (downcast-unsigned
             256
             255
             (if (elt-ref (var-ref %s.164) is_some 0)
                 (+ (tunsigned 256)
                    (safe-cast
                      (tunsigned 256)
                      (tunsigned 255)
                      (var-ref %a.165))
                    (safe-cast (tunsigned 256) (tunsigned 1) '1))
                 (safe-cast
                   (tunsigned 256)
                   (tunsigned 255)
                   (var-ref %a.165))))))
       ((safe-cast (tunsigned 255) (tunsigned 0) '0)
         (tunsigned 255))
       ((var-ref %sigs.166)
         (tvector
           9
           (tstruct
             Maybe
             (is_some (tboolean))
             (value
               (tstruct
                 ValidatorSignature
                 (pk (tpoint (curve-jubjub)))
                 (r (tpoint (curve-jubjub)))
                 (s (tfield (field-native)))))))
         (tstruct
           Maybe
           (is_some (tboolean))
           (value
             (tstruct
               ValidatorSignature
               (pk (tpoint (curve-jubjub)))
               (r (tpoint (curve-jubjub)))
               (s (tfield (field-native))))))))))
 (circuit %claim_deposit.128 (exported #t) (pure #f) (proof #t)
   ((%salt.167 (tbytes 32))) (ttuple)
   (seq (let* (((%key.168 (tbytes 32)) (var-ref %salt.167)))
          (seq (assert
                 (public-ledger %unclaimed_deposits.119 read (2) member (tboolean)
                   (instructions (dup (n 0))
                     (idx (cached #f) (pushPath #f) (path ((align 2 1))))
                     (push
                       (storage #f)
                       (value (state-value cell (var-ref %key.168))))
                     (member) (popeq (cached #t) (result (void))))
                   (var-ref %key.168))
                 "claim_deposit: no deposit")
               (public-ledger %unclaimed_deposits.119 remove (2) remove (ttuple)
                 (instructions
                   (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                   (push
                     (storage #f)
                     (value (state-value cell (var-ref %key.168))))
                   (rem (cached #f))
                   (ins (cached #t) (n 1)))
                 (var-ref %key.168))))
        (return (tuple))))
 (circuit %witness_deposit.114 (exported #t) (pure #f) (proof #t)
   ((%sigs.154
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native))))))))
     (%channel_id.157 (tbytes 32))
     (%amount.155
       (tunsigned 340282366920938463463374607431768211455))
     (%token_ref.156 (tbytes 32)))
   (ttuple)
   (seq (assert
          (let* (((%t.72 (tunsigned 255)) (call
                                            %count_valid_sig.137
                                            (var-ref %sigs.154))))
            (>= 8
                (var-ref %t.72)
                (public-ledger %threshold.118 read (0) read (tunsigned 255)
                  (instructions
                    (dup (n 0))
                    (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                    (popeq (cached #f) (result (void)))))))
          "witness_deposit: threshold")
        (let* (((%tmp.158
                  (tstruct
                    UnclaimedDeposit
                    (amount
                      (tunsigned 340282366920938463463374607431768211455))
                    (token_ref (tbytes 32)))) (new (tstruct
                                                     UnclaimedDeposit
                                                     (amount
                                                       (tunsigned
                                                         340282366920938463463374607431768211455))
                                                     (token_ref
                                                       (tbytes 32)))
                                                   (var-ref %amount.155)
                                                   (var-ref
                                                     %token_ref.156))))
          (public-ledger %unclaimed_deposits.119 update (2) insert (ttuple)
            (instructions (idx (cached #f) (pushPath #t) (path ((align 2 1))))
              (push
                (storage #f)
                (value (state-value cell (var-ref %channel_id.157))))
              (push
                (storage #t)
                (value
                  (state-value
                    ADT
                    (var-ref %tmp.158)
                    (tstruct
                      UnclaimedDeposit
                      (amount
                        (tunsigned
                          340282366920938463463374607431768211455))
                      (token_ref (tbytes 32))))))
              (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
            (var-ref %channel_id.157) (var-ref %tmp.158)))
        (return (tuple))))
 (circuit %witness_egress.115 (exported #t) (pure #f) (proof #t)
   ((%sigs.163
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native))))))))
     (%job_id.159
       (tunsigned 340282366920938463463374607431768211455)))
   (ttuple)
   (seq (seq (assert
               (let* (((%t.109 (tunsigned 255)) (call
                                                  %count_valid_sig.137
                                                  (var-ref %sigs.163))))
                 (>= 8
                     (var-ref %t.109)
                     (public-ledger %threshold.118 read (0) read (tunsigned 255)
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 0 1))))
                         (popeq (cached #f) (result (void)))))))
               "witness_egress: threshold")
             (let* (((%key.160 (tfield (field-native))) (safe-cast
                                                          (tfield
                                                            (field-native))
                                                          (tunsigned
                                                            340282366920938463463374607431768211455)
                                                          (var-ref
                                                            %job_id.159))))
               (seq (assert
                      (public-ledger %egress_jobs.129 read (4) member (tboolean)
                        (instructions (dup (n 0))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 4 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %key.160))))
                          (member) (popeq (cached #t) (result (void))))
                        (var-ref %key.160))
                      "witness_egress: unknown job")
                    (let* (((%job.161
                              (tstruct EgressJob
                                (id (tunsigned
                                      340282366920938463463374607431768211455))
                                (destination (tbytes 32))
                                (token_ref (tbytes 32))
                                (amount
                                  (tunsigned
                                    340282366920938463463374607431768211455))
                                (status
                                  (tenum JobStatus pending completed)))) (public-ledger
                                                                           %egress_jobs.129
                                                                           read
                                                                           (4)
                                                                           lookup
                                                                           (tstruct
                                                                             EgressJob
                                                                             (id (tunsigned
                                                                                   340282366920938463463374607431768211455))
                                                                             (destination
                                                                               (tbytes
                                                                                 32))
                                                                             (token_ref
                                                                               (tbytes
                                                                                 32))
                                                                             (amount
                                                                               (tunsigned
                                                                                 340282366920938463463374607431768211455))
                                                                             (status
                                                                               (tenum
                                                                                 JobStatus
                                                                                 pending
                                                                                 completed)))
                                                                           (instructions
                                                                             (dup (n 0))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((align
                                                                                       4
                                                                                       1))))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((var-ref
                                                                                       %key.160))))
                                                                             (popeq
                                                                               (cached
                                                                                 #f)
                                                                               (result
                                                                                 (void))))
                                                                           (var-ref
                                                                             %key.160))))
                      (let* (((%tmp.162
                                (tstruct EgressJob
                                  (id (tunsigned
                                        340282366920938463463374607431768211455))
                                  (destination (tbytes 32))
                                  (token_ref (tbytes 32))
                                  (amount
                                    (tunsigned
                                      340282366920938463463374607431768211455))
                                  (status
                                    (tenum JobStatus pending completed)))) (new (tstruct
                                                                                  EgressJob
                                                                                  (id (tunsigned
                                                                                        340282366920938463463374607431768211455))
                                                                                  (destination
                                                                                    (tbytes
                                                                                      32))
                                                                                  (token_ref
                                                                                    (tbytes
                                                                                      32))
                                                                                  (amount
                                                                                    (tunsigned
                                                                                      340282366920938463463374607431768211455))
                                                                                  (status
                                                                                    (tenum
                                                                                      JobStatus
                                                                                      pending
                                                                                      completed)))
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.161)
                                                                                  id
                                                                                  0)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.161)
                                                                                  destination
                                                                                  1)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.161)
                                                                                  token_ref
                                                                                  2)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.161)
                                                                                  amount
                                                                                  3)
                                                                                (enum-ref
                                                                                  (tenum
                                                                                    JobStatus
                                                                                    pending
                                                                                    completed)
                                                                                  completed))))
                        (public-ledger %egress_jobs.129 update (4) insert (ttuple)
                          (instructions
                            (idx (cached #f)
                                 (pushPath #t)
                                 (path ((align 4 1))))
                            (push
                              (storage #f)
                              (value
                                (state-value cell (var-ref %key.160))))
                            (push
                              (storage #t)
                              (value
                                (state-value
                                  ADT
                                  (var-ref %tmp.162)
                                  (tstruct EgressJob
                                    (id (tunsigned
                                          340282366920938463463374607431768211455))
                                    (destination (tbytes 32))
                                    (token_ref (tbytes 32))
                                    (amount
                                      (tunsigned
                                        340282366920938463463374607431768211455))
                                    (status
                                      (tenum
                                        JobStatus
                                        pending
                                        completed))))))
                            (ins (cached #f) (n 1))
                            (ins (cached #t) (n 1)))
                          (var-ref %key.160) (var-ref %tmp.162)))))))
        (return (tuple))))
 (circuit %withdraw.117 (exported #t) (pure #f) (proof #t)
   ((%coin.141
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%destination.142 (tbytes 32)))
   (tunsigned 340282366920938463463374607431768211455)
   (seq (call %receiveShielded.145 (var-ref %coin.141))
        (let* (((%id.139
                  (tunsigned 340282366920938463463374607431768211455)) (safe-cast
                                                                         (tunsigned
                                                                           340282366920938463463374607431768211455)
                                                                         (tunsigned
                                                                           18446744073709551615)
                                                                         (public-ledger
                                                                           %next_job_id.124
                                                                           read
                                                                           (3)
                                                                           read
                                                                           (tunsigned
                                                                             18446744073709551615)
                                                                           (instructions
                                                                             (dup (n 0))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((align
                                                                                       3
                                                                                       1))))
                                                                             (popeq
                                                                               (cached
                                                                                 #t)
                                                                               (result
                                                                                 (void))))))))
          (seq (let* (((%tmp.140 (tunsigned 65535)) (safe-cast
                                                      (tunsigned 65535)
                                                      (tunsigned 1)
                                                      '1)))
                 (public-ledger %next_job_id.124 update (3) increment (ttuple)
                   (instructions
                     (idx (cached #f) (pushPath #t) (path ((align 3 1))))
                     (addi (immediate (value->int (var-ref %tmp.140))))
                     (ins (cached #t) (n 1)))
                   (var-ref %tmp.140)))
               (let* (((%tmp.143 (tfield (field-native))) (safe-cast
                                                            (tfield
                                                              (field-native))
                                                            (tunsigned
                                                              340282366920938463463374607431768211455)
                                                            (var-ref
                                                              %id.139))))
                 (let* (((%tmp.144
                           (tstruct EgressJob
                             (id (tunsigned
                                   340282366920938463463374607431768211455))
                             (destination (tbytes 32))
                             (token_ref (tbytes 32))
                             (amount
                               (tunsigned
                                 340282366920938463463374607431768211455))
                             (status (tenum JobStatus pending completed)))) (new (tstruct
                                                                                   EgressJob
                                                                                   (id (tunsigned
                                                                                         340282366920938463463374607431768211455))
                                                                                   (destination
                                                                                     (tbytes
                                                                                       32))
                                                                                   (token_ref
                                                                                     (tbytes
                                                                                       32))
                                                                                   (amount
                                                                                     (tunsigned
                                                                                       340282366920938463463374607431768211455))
                                                                                   (status
                                                                                     (tenum
                                                                                       JobStatus
                                                                                       pending
                                                                                       completed)))
                                                                                 (var-ref
                                                                                   %id.139)
                                                                                 (var-ref
                                                                                   %destination.142)
                                                                                 (public-ledger
                                                                                   %fee_token.126
                                                                                   read
                                                                                   (7)
                                                                                   read
                                                                                   (tbytes
                                                                                     32)
                                                                                   (instructions
                                                                                     (dup (n 0))
                                                                                     (idx (cached
                                                                                            #f)
                                                                                          (pushPath
                                                                                            #f)
                                                                                          (path
                                                                                            ((align
                                                                                               7
                                                                                               1))))
                                                                                     (popeq
                                                                                       (cached
                                                                                         #f)
                                                                                       (result
                                                                                         (void)))))
                                                                                 (elt-ref
                                                                                   (var-ref
                                                                                     %coin.141)
                                                                                   value
                                                                                   2)
                                                                                 (enum-ref
                                                                                   (tenum
                                                                                     JobStatus
                                                                                     pending
                                                                                     completed)
                                                                                   pending))))
                   (public-ledger %egress_jobs.129 update (4) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 4 1))))
                       (push
                         (storage #f)
                         (value (state-value cell (var-ref %tmp.143))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             ADT
                             (var-ref %tmp.144)
                             (tstruct EgressJob
                               (id (tunsigned
                                     340282366920938463463374607431768211455))
                               (destination (tbytes 32))
                               (token_ref (tbytes 32))
                               (amount
                                 (tunsigned
                                   340282366920938463463374607431768211455))
                               (status
                                 (tenum JobStatus pending completed))))))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
                     (var-ref %tmp.143) (var-ref %tmp.144))))
               (return (var-ref %id.139))))))
 (circuit %sign.123 (exported #t) (pure #f) (proof #t)
   ((%payload.148 (tbytes 32))
     (%domain_id.150 (tunsigned 255))
     (%salt.149 (tbytes 32))
     (%fee_coin.153
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455)))))
   (tunsigned 340282366920938463463374607431768211455)
   (seq (call %receiveShielded.145 (var-ref %fee_coin.153))
        (let* (((%id.146
                  (tunsigned 340282366920938463463374607431768211455)) (safe-cast
                                                                         (tunsigned
                                                                           340282366920938463463374607431768211455)
                                                                         (tunsigned
                                                                           18446744073709551615)
                                                                         (public-ledger
                                                                           %next_signing_request_id.125
                                                                           read
                                                                           (8)
                                                                           read
                                                                           (tunsigned
                                                                             18446744073709551615)
                                                                           (instructions
                                                                             (dup (n 0))
                                                                             (idx (cached
                                                                                    #f)
                                                                                  (pushPath
                                                                                    #f)
                                                                                  (path
                                                                                    ((align
                                                                                       8
                                                                                       1))))
                                                                             (popeq
                                                                               (cached
                                                                                 #t)
                                                                               (result
                                                                                 (void))))))))
          (seq (let* (((%tmp.147 (tunsigned 65535)) (safe-cast
                                                      (tunsigned 65535)
                                                      (tunsigned 1)
                                                      '1)))
                 (public-ledger %next_signing_request_id.125 update (8) increment
                   (ttuple)
                   (instructions
                     (idx (cached #f) (pushPath #t) (path ((align 8 1))))
                     (addi (immediate (value->int (var-ref %tmp.147))))
                     (ins (cached #t) (n 1)))
                   (var-ref %tmp.147)))
               (let* (((%tmp.151 (tfield (field-native))) (safe-cast
                                                            (tfield
                                                              (field-native))
                                                            (tunsigned
                                                              340282366920938463463374607431768211455)
                                                            (var-ref
                                                              %id.146))))
                 (let* (((%tmp.152
                           (tstruct SigningRequest (entity_id (tbytes 32))
                             (domain_id (tunsigned 255))
                             (payload (tbytes 32))
                             (status
                               (tenum
                                 SigningRequestStatus
                                 pending
                                 fulfilled))
                             (signature (tbytes 64)))) (new (tstruct
                                                              SigningRequest
                                                              (entity_id
                                                                (tbytes
                                                                  32))
                                                              (domain_id
                                                                (tunsigned
                                                                  255))
                                                              (payload
                                                                (tbytes
                                                                  32))
                                                              (status
                                                                (tenum
                                                                  SigningRequestStatus
                                                                  pending
                                                                  fulfilled))
                                                              (signature
                                                                (tbytes
                                                                  64)))
                                                            (var-ref
                                                              %salt.149)
                                                            (var-ref
                                                              %domain_id.150)
                                                            (var-ref
                                                              %payload.148)
                                                            (enum-ref
                                                              (tenum
                                                                SigningRequestStatus
                                                                pending
                                                                fulfilled)
                                                              pending)
                                                            (default
                                                              (tbytes
                                                                64)))))
                   (public-ledger %signing_requests.121 update (9) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 9 1))))
                       (push
                         (storage #f)
                         (value (state-value cell (var-ref %tmp.151))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             ADT
                             (var-ref %tmp.152)
                             (tstruct SigningRequest (entity_id (tbytes 32))
                               (domain_id (tunsigned 255))
                               (payload (tbytes 32))
                               (status
                                 (tenum
                                   SigningRequestStatus
                                   pending
                                   fulfilled))
                               (signature (tbytes 64))))))
                       (ins (cached #f) (n 1)) (ins (cached #t) (n 1)))
                     (var-ref %tmp.151) (var-ref %tmp.152))))
               (return (var-ref %id.146))))))
 (circuit %fulfill_signing_request.127 (exported #t) (pure #f)
   (proof #t)
   ((%sigs.138
      (tvector
        9
        (tstruct
          Maybe
          (is_some (tboolean))
          (value
            (tstruct
              ValidatorSignature
              (pk (tpoint (curve-jubjub)))
              (r (tpoint (curve-jubjub)))
              (s (tfield (field-native))))))))
     (%request_id.130
       (tunsigned 340282366920938463463374607431768211455))
     (%signature.133 (tbytes 64)))
   (ttuple)
   (seq (seq (assert
               (let* (((%t.34 (tunsigned 255)) (call
                                                 %count_valid_sig.137
                                                 (var-ref %sigs.138))))
                 (>= 8
                     (var-ref %t.34)
                     (public-ledger %threshold.118 read (0) read (tunsigned 255)
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 0 1))))
                         (popeq (cached #f) (result (void)))))))
               "fulfill: threshold")
             (let* (((%key.131 (tfield (field-native))) (safe-cast
                                                          (tfield
                                                            (field-native))
                                                          (tunsigned
                                                            340282366920938463463374607431768211455)
                                                          (var-ref
                                                            %request_id.130))))
               (seq (assert
                      (public-ledger %signing_requests.121 read (9) member (tboolean)
                        (instructions (dup (n 0))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 9 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %key.131))))
                          (member) (popeq (cached #t) (result (void))))
                        (var-ref %key.131))
                      "fulfill: unknown request")
                    (let* (((%req.135
                              (tstruct SigningRequest (entity_id (tbytes 32))
                                (domain_id (tunsigned 255))
                                (payload (tbytes 32))
                                (status
                                  (tenum
                                    SigningRequestStatus
                                    pending
                                    fulfilled))
                                (signature (tbytes 64)))) (public-ledger
                                                            %signing_requests.121
                                                            read (9) lookup
                                                            (tstruct
                                                              SigningRequest
                                                              (entity_id
                                                                (tbytes
                                                                  32))
                                                              (domain_id
                                                                (tunsigned
                                                                  255))
                                                              (payload
                                                                (tbytes
                                                                  32))
                                                              (status
                                                                (tenum
                                                                  SigningRequestStatus
                                                                  pending
                                                                  fulfilled))
                                                              (signature
                                                                (tbytes
                                                                  64)))
                                                            (instructions
                                                              (dup (n 0))
                                                              (idx (cached
                                                                     #f)
                                                                   (pushPath
                                                                     #f)
                                                                   (path
                                                                     ((align
                                                                        9
                                                                        1))))
                                                              (idx (cached
                                                                     #f)
                                                                   (pushPath
                                                                     #f)
                                                                   (path
                                                                     ((var-ref
                                                                        %key.131))))
                                                              (popeq
                                                                (cached #f)
                                                                (result
                                                                  (void))))
                                                            (var-ref
                                                              %key.131))))
                      (seq (let* (((%tmp.136
                                     (tstruct SigningRequest
                                       (entity_id (tbytes 32))
                                       (domain_id (tunsigned 255))
                                       (payload (tbytes 32))
                                       (status
                                         (tenum
                                           SigningRequestStatus
                                           pending
                                           fulfilled))
                                       (signature (tbytes 64)))) (new (tstruct
                                                                        SigningRequest
                                                                        (entity_id
                                                                          (tbytes
                                                                            32))
                                                                        (domain_id
                                                                          (tunsigned
                                                                            255))
                                                                        (payload
                                                                          (tbytes
                                                                            32))
                                                                        (status
                                                                          (tenum
                                                                            SigningRequestStatus
                                                                            pending
                                                                            fulfilled))
                                                                        (signature
                                                                          (tbytes
                                                                            64)))
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.135)
                                                                        entity_id
                                                                        0)
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.135)
                                                                        domain_id
                                                                        1)
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.135)
                                                                        payload
                                                                        2)
                                                                      (enum-ref
                                                                        (tenum
                                                                          SigningRequestStatus
                                                                          pending
                                                                          fulfilled)
                                                                        fulfilled)
                                                                      (var-ref
                                                                        %signature.133))))
                             (public-ledger %signing_requests.121 update (9) insert
                               (ttuple)
                               (instructions
                                 (idx (cached #f)
                                      (pushPath #t)
                                      (path ((align 9 1))))
                                 (push
                                   (storage #f)
                                   (value
                                     (state-value
                                       cell
                                       (var-ref %key.131))))
                                 (push
                                   (storage #t)
                                   (value
                                     (state-value
                                       ADT
                                       (var-ref %tmp.136)
                                       (tstruct SigningRequest
                                         (entity_id (tbytes 32))
                                         (domain_id (tunsigned 255))
                                         (payload (tbytes 32))
                                         (status
                                           (tenum
                                             SigningRequestStatus
                                             pending
                                             fulfilled))
                                         (signature (tbytes 64))))))
                                 (ins (cached #f) (n 1))
                                 (ins (cached #t) (n 1)))
                               (var-ref %key.131) (var-ref %tmp.136)))
                           (let* (((%tmp.134 (tbytes 32)) (call
                                                            %persistentHash.132
                                                            (var-ref
                                                              %signature.133))))
                             (public-ledger %processed_attestations.122 update (5)
                               insert (ttuple)
                               (instructions
                                 (idx (cached #f)
                                      (pushPath #t)
                                      (path ((align 5 1))))
                                 (push
                                   (storage #f)
                                   (value
                                     (state-value
                                       cell
                                       (var-ref %tmp.134))))
                                 (push
                                   (storage #t)
                                   (value (state-value null)))
                                 (ins (cached #f) (n 1))
                                 (ins (cached #t) (n 1)))
                               (var-ref %tmp.134))))))))
        (return (tuple)))))
