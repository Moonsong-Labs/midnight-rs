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
 (kernel-declaration (%kernel.182 () (exported #f) (Kernel)))
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
 (circuit %right.178 (exported #f) (pure #t) (proof #f)
   ((%value.179 (tstruct ContractAddress (bytes (tbytes 32)))))
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
          (var-ref %value.179))))
 (circuit %receiveShielded.145 (exported #f) (pure #f) (proof #f)
   ((%coin.180
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455)))))
   (ttuple)
   (seq (let* (((%recipient.181
                  (tstruct
                    Either
                    (is_left (tboolean))
                    (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
                    (right (tstruct ContractAddress (bytes (tbytes 32)))))) (call
                                                                              %right.178
                                                                              (public-ledger
                                                                                %kernel.182
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
                 %createZswapOutput.170
                 (var-ref %coin.180)
                 (var-ref %recipient.181))
               (let* (((%tmp.183 (tbytes 32)) (call
                                                %coinCommitment.173
                                                (var-ref %coin.180)
                                                (var-ref %recipient.181))))
                 (public-ledger %kernel.182 () claimZswapCoinReceive (ttuple)
                   (instructions (swap (n 0))
                     (idx (cached #t) (pushPath #t) (path ((align 1 1))))
                     (push
                       (storage #f)
                       (value (state-value cell (var-ref %tmp.183))))
                     (push (storage #f) (value (state-value null)))
                     (ins (cached #t) (n 2)) (swap (n 0)))
                   (var-ref %tmp.183)))))
        (return (tuple))))
 (circuit %coinCommitment.173 (exported #f) (pure #t) (proof #f)
   ((%coin.174
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%recipient.175
       (tstruct
         Either
         (is_left (tboolean))
         (left (tstruct ZswapCoinPublicKey (bytes (tbytes 32))))
         (right (tstruct ContractAddress (bytes (tbytes 32)))))))
   (tbytes 32)
   (return
     (call
       %persistentHash.176
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
            (var-ref %coin.174)
            (elt-ref (var-ref %recipient.175) is_left 0)
            (if (elt-ref (var-ref %recipient.175) is_left 0)
                (elt-ref (elt-ref (var-ref %recipient.175) left 1) bytes 0)
                (elt-ref
                  (elt-ref (var-ref %recipient.175) right 2)
                  bytes
                  0))))))
 (native
   %persistentHash.176
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.177
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
 (native
   %persistentHash.136
   (entry "__compactRuntime.persistentHash" circuit)
   ((%value.169 (tbytes 64)))
   (tbytes 32))
 (native
   %createZswapOutput.170
   (entry "__compactRuntime.createZswapOutput" witness)
   ((%coin.171
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
   (ttuple))
 (circuit %count_valid_sig.138 (exported #f) (pure #t) (proof #f)
   ((%sigs.164
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
           (%s.166
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
             (if (elt-ref (var-ref %s.166) is_some 0)
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
       ((var-ref %sigs.164)
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
                 (public-ledger %unclaimed_deposits.119 (2) member (tboolean)
                   (instructions (dup (n 0))
                     (idx (cached #f) (pushPath #f) (path ((align 2 1))))
                     (push
                       (storage #f)
                       (value (state-value cell (var-ref %key.168))))
                     (member) (popeq (cached #t) (result (void))))
                   (var-ref %key.168))
                 "claim_deposit: no deposit")
               (public-ledger %unclaimed_deposits.119 (2) remove (ttuple)
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
   ((%sigs.156
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
     (%amount.154
       (tunsigned 340282366920938463463374607431768211455))
     (%token_ref.155 (tbytes 32)))
   (ttuple)
   (seq (assert
          (let* (((%t.72 (tunsigned 255)) (call
                                            %count_valid_sig.138
                                            (var-ref %sigs.156))))
            (>= 8
                (var-ref %t.72)
                (public-ledger %threshold.118 (0) read (tunsigned 255)
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
                                                   (var-ref %amount.154)
                                                   (var-ref
                                                     %token_ref.155))))
          (public-ledger %unclaimed_deposits.119 (2) insert (ttuple)
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
   ((%sigs.159
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
     (%job_id.160
       (tunsigned 340282366920938463463374607431768211455)))
   (ttuple)
   (seq (seq (assert
               (let* (((%t.109 (tunsigned 255)) (call
                                                  %count_valid_sig.138
                                                  (var-ref %sigs.159))))
                 (>= 8
                     (var-ref %t.109)
                     (public-ledger %threshold.118 (0) read (tunsigned 255)
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 0 1))))
                         (popeq (cached #f) (result (void)))))))
               "witness_egress: threshold")
             (let* (((%key.161 (tfield (field-native))) (safe-cast
                                                          (tfield
                                                            (field-native))
                                                          (tunsigned
                                                            340282366920938463463374607431768211455)
                                                          (var-ref
                                                            %job_id.160))))
               (seq (assert
                      (public-ledger %egress_jobs.129 (4) member (tboolean)
                        (instructions (dup (n 0))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 4 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %key.161))))
                          (member) (popeq (cached #t) (result (void))))
                        (var-ref %key.161))
                      "witness_egress: unknown job")
                    (let* (((%job.162
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
                                                                                       %key.161))))
                                                                             (popeq
                                                                               (cached
                                                                                 #f)
                                                                               (result
                                                                                 (void))))
                                                                           (var-ref
                                                                             %key.161))))
                      (let* (((%tmp.163
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
                                                                                    %job.162)
                                                                                  id
                                                                                  0)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.162)
                                                                                  destination
                                                                                  1)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.162)
                                                                                  token_ref
                                                                                  2)
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %job.162)
                                                                                  amount
                                                                                  3)
                                                                                (enum-ref
                                                                                  (tenum
                                                                                    JobStatus
                                                                                    pending
                                                                                    completed)
                                                                                  completed))))
                        (public-ledger %egress_jobs.129 (4) insert (ttuple)
                          (instructions
                            (idx (cached #f)
                                 (pushPath #t)
                                 (path ((align 4 1))))
                            (push
                              (storage #f)
                              (value
                                (state-value cell (var-ref %key.161))))
                            (push
                              (storage #t)
                              (value
                                (state-value
                                  ADT
                                  (var-ref %tmp.163)
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
                          (var-ref %key.161) (var-ref %tmp.163)))))))
        (return (tuple))))
 (circuit %withdraw.117 (exported #t) (pure #f) (proof #t)
   ((%coin.139
      (tstruct
        ShieldedCoinInfo
        (nonce (tbytes 32))
        (color (tbytes 32))
        (value
          (tunsigned 340282366920938463463374607431768211455))))
     (%destination.140 (tbytes 32)))
   (tunsigned 340282366920938463463374607431768211455)
   (seq (call %receiveShielded.145 (var-ref %coin.139))
        (let* (((%id.141
                  (tunsigned 340282366920938463463374607431768211455)) (safe-cast
                                                                         (tunsigned
                                                                           340282366920938463463374607431768211455)
                                                                         (tunsigned
                                                                           18446744073709551615)
                                                                         (public-ledger
                                                                           %next_job_id.124
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
          (seq (let* (((%tmp.142 (tunsigned 65535)) (safe-cast
                                                      (tunsigned 65535)
                                                      (tunsigned 1)
                                                      '1)))
                 (public-ledger %next_job_id.124 (3) increment (ttuple)
                   (instructions
                     (idx (cached #f) (pushPath #t) (path ((align 3 1))))
                     (addi (immediate (value->int (var-ref %tmp.142))))
                     (ins (cached #t) (n 1)))
                   (var-ref %tmp.142)))
               (let* (((%tmp.143 (tfield (field-native))) (safe-cast
                                                            (tfield
                                                              (field-native))
                                                            (tunsigned
                                                              340282366920938463463374607431768211455)
                                                            (var-ref
                                                              %id.141))))
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
                                                                                   %id.141)
                                                                                 (var-ref
                                                                                   %destination.140)
                                                                                 (public-ledger
                                                                                   %fee_token.126
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
                                                                                     %coin.139)
                                                                                   value
                                                                                   2)
                                                                                 (enum-ref
                                                                                   (tenum
                                                                                     JobStatus
                                                                                     pending
                                                                                     completed)
                                                                                   pending))))
                   (public-ledger %egress_jobs.129 (4) insert (ttuple)
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
               (return (var-ref %id.141))))))
 (circuit %sign.123 (exported #t) (pure #f) (proof #t)
   ((%payload.148 (tbytes 32))
     (%domain_id.149 (tunsigned 255))
     (%salt.146 (tbytes 32))
     (%fee_coin.147
       (tstruct
         ShieldedCoinInfo
         (nonce (tbytes 32))
         (color (tbytes 32))
         (value
           (tunsigned 340282366920938463463374607431768211455)))))
   (tunsigned 340282366920938463463374607431768211455)
   (seq (call %receiveShielded.145 (var-ref %fee_coin.147))
        (let* (((%id.150
                  (tunsigned 340282366920938463463374607431768211455)) (safe-cast
                                                                         (tunsigned
                                                                           340282366920938463463374607431768211455)
                                                                         (tunsigned
                                                                           18446744073709551615)
                                                                         (public-ledger
                                                                           %next_signing_request_id.125
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
          (seq (let* (((%tmp.151 (tunsigned 65535)) (safe-cast
                                                      (tunsigned 65535)
                                                      (tunsigned 1)
                                                      '1)))
                 (public-ledger %next_signing_request_id.125 (8) increment (ttuple)
                   (instructions
                     (idx (cached #f) (pushPath #t) (path ((align 8 1))))
                     (addi (immediate (value->int (var-ref %tmp.151))))
                     (ins (cached #t) (n 1)))
                   (var-ref %tmp.151)))
               (let* (((%tmp.152 (tfield (field-native))) (safe-cast
                                                            (tfield
                                                              (field-native))
                                                            (tunsigned
                                                              340282366920938463463374607431768211455)
                                                            (var-ref
                                                              %id.150))))
                 (let* (((%tmp.153
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
                                                              %salt.146)
                                                            (var-ref
                                                              %domain_id.149)
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
                   (public-ledger %signing_requests.121 (9) insert (ttuple)
                     (instructions (idx (cached #f) (pushPath #t) (path ((align 9 1))))
                       (push
                         (storage #f)
                         (value (state-value cell (var-ref %tmp.152))))
                       (push
                         (storage #t)
                         (value
                           (state-value
                             ADT
                             (var-ref %tmp.153)
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
                     (var-ref %tmp.152) (var-ref %tmp.153))))
               (return (var-ref %id.150))))))
 (circuit %fulfill_signing_request.127 (exported #t) (pure #f)
   (proof #t)
   ((%sigs.131
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
     (%request_id.132
       (tunsigned 340282366920938463463374607431768211455))
     (%signature.130 (tbytes 64)))
   (ttuple)
   (seq (seq (assert
               (let* (((%t.34 (tunsigned 255)) (call
                                                 %count_valid_sig.138
                                                 (var-ref %sigs.131))))
                 (>= 8
                     (var-ref %t.34)
                     (public-ledger %threshold.118 (0) read (tunsigned 255)
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 0 1))))
                         (popeq (cached #f) (result (void)))))))
               "fulfill: threshold")
             (let* (((%key.133 (tfield (field-native))) (safe-cast
                                                          (tfield
                                                            (field-native))
                                                          (tunsigned
                                                            340282366920938463463374607431768211455)
                                                          (var-ref
                                                            %request_id.132))))
               (seq (assert
                      (public-ledger %signing_requests.121 (9) member (tboolean)
                        (instructions (dup (n 0))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 9 1))))
                          (push
                            (storage #f)
                            (value (state-value cell (var-ref %key.133))))
                          (member) (popeq (cached #t) (result (void))))
                        (var-ref %key.133))
                      "fulfill: unknown request")
                    (let* (((%req.134
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
                                                            (9) lookup
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
                                                                        %key.133))))
                                                              (popeq
                                                                (cached #f)
                                                                (result
                                                                  (void))))
                                                            (var-ref
                                                              %key.133))))
                      (seq (let* (((%tmp.137
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
                                                                          %req.134)
                                                                        entity_id
                                                                        0)
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.134)
                                                                        domain_id
                                                                        1)
                                                                      (elt-ref
                                                                        (var-ref
                                                                          %req.134)
                                                                        payload
                                                                        2)
                                                                      (enum-ref
                                                                        (tenum
                                                                          SigningRequestStatus
                                                                          pending
                                                                          fulfilled)
                                                                        fulfilled)
                                                                      (var-ref
                                                                        %signature.130))))
                             (public-ledger %signing_requests.121 (9) insert (ttuple)
                               (instructions
                                 (idx (cached #f)
                                      (pushPath #t)
                                      (path ((align 9 1))))
                                 (push
                                   (storage #f)
                                   (value
                                     (state-value
                                       cell
                                       (var-ref %key.133))))
                                 (push
                                   (storage #t)
                                   (value
                                     (state-value
                                       ADT
                                       (var-ref %tmp.137)
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
                               (var-ref %key.133) (var-ref %tmp.137)))
                           (let* (((%tmp.135 (tbytes 32)) (call
                                                            %persistentHash.136
                                                            (var-ref
                                                              %signature.130))))
                             (public-ledger %processed_attestations.122 (5) insert
                               (ttuple)
                               (instructions
                                 (idx (cached #f)
                                      (pushPath #t)
                                      (path ((align 5 1))))
                                 (push
                                   (storage #f)
                                   (value
                                     (state-value
                                       cell
                                       (var-ref %tmp.135))))
                                 (push
                                   (storage #t)
                                   (value (state-value null)))
                                 (ins (cached #f) (n 1))
                                 (ins (cached #t) (n 1)))
                               (var-ref %tmp.135))))))))
        (return (tuple)))))
