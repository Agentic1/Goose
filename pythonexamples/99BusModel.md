                +------------------  “Public Lane”  ------------------+
                |                                                     |
     ┌──────────▼──────────┐    (raw, open envelopes)    ┌────────────▼────────────┐
     |  Public  Producers  |  ─────────────────────────▶ |  Public  Consumers      |
     |  (edge in, agents)  |        FREE TRAFFIC         |  (sniffer, dev tools)   |
     └──────────▲──────────┘                             └────────────▲────────────┘
                |                             ⬇ usage-analytics feed*
                |                             $  (behavioural-data sale)
+============== CLEARNING HOUSE BUS ==============+ 
|                                                 |      Legend
|  ╔═ Premium Encrypted Track ═╗                   |      $  = revenue event
|  |                           |                  |      *  = optional feature
|  |  ┌──────────┐   secure    |     decrypt      |                         
|  |  |  Sender   |──────$────▶|┌───────────────┐ |                      
|  |  |  (Demand) |  union-fee |│  Validator(s) │ |                       
|  |  └────┬─────┘             |└───────────────┘ |                       
|  |       │   rebate $        |        ▲ attested log *                  
|  |       ▼                   |        | (BLS/Merkle)                    
|  |  ┌──────────┐ response $  |        |                                   
|  |  | Provider  |◀───────────┘        |                                   
|  |  | (Supply)  |<───── service ──────┘                                   
|  |  └──────────┘                       │                                   
|  ╚═════════════════════════════════════╝   $ treasury share               
|                                                 |                         
+=================================================+                         
              ▲                    ▲                                    
              | edge-out msgs $    | inbound edge events $                
      marketplace connectors  *    | (Telegram, GSM, etc.)                

